//! Zeron v3's split chats (ui/screens/shell/split.luau, page.luau), and
//! Zeron v2, the single-page layout, as the window runs them: mounted
//! headless on the shell's fixture (ui/shell_fixture.luau).
//!
//! A chat dragged from the sidebar onto a chat's edge sits beside it; a
//! click focuses a pane; ⌘W takes it out; the split is kept across a
//! reload and a launch; and resizing the window moves the panes without
//! running any more script than one chat does. Zeron v2 (installed as
//! `live` installs it, composed into `.live` as a switch composes it) has
//! none of that: one page, no cards; and switching back gives v3 again.
//!
//! (The Lua test kit can't reload or relaunch; the host can.)

use std::path::{Path, PathBuf};

use zgx::{
    input::{InputEvent, Modifiers, PointerButton},
    scene::Rect,
};
use zgx_script::{ScriptHost, testing::TestApp};

use crate::plugins::{live, official, preview};

const W: f32 = 1400.;
const H: f32 = 860.;

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-split-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The shell's fixture at `fixture`, its saved state in `storage`.
fn mount_at(fixture: &Path, storage: &Path) -> TestApp {
    let (fixture, storage) = (fixture.to_path_buf(), storage.to_path_buf());
    let app = TestApp::new(move |runtime| ScriptHost::new(runtime, fixture).storage(storage));
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    app
}

fn mount(storage: &Path) -> TestApp {
    mount_at(&ui().join("shell_fixture.luau"), storage)
}

fn open(app: &mut TestApp, title: &str) {
    app.click(&format!("#shell-sidebar >> role:button \"{title}\""));
    app.frames(20);
}

/// The whole interface, a node a line (`tree` stops at 200).
fn full_tree(app: &mut TestApp) -> String {
    app.ui.prepare_frame();
    let opts = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: false };
    let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, None, &opts);
    zgx_script::driver::tree_lines_text(&lines, hidden)
}

/// The panes showing (none for a chat alone).
fn panes(app: &mut TestApp) -> usize {
    shown_panes(app).len()
}

fn pointer(app: &mut TestApp, event: InputEvent) {
    app.ui.dispatch_with_modifiers(event, Modifiers::default());
    app.frames(2);
}

/// Pressed at `from`, carried past the drag threshold to `to` in steps, a
/// frame between each (the panes' drop zones show once a drag starts),
/// held there for `during`, then let go.
fn carry(app: &mut TestApp, from: (f32, f32), to: (f32, f32), during: impl FnOnce(&mut TestApp)) {
    let ((sx, sy), (x, y)) = (from, to);
    let button = PointerButton::Primary;
    pointer(app, InputEvent::PointerMove { x: sx, y: sy });
    pointer(app, InputEvent::PointerDown { x: sx, y: sy, button });
    pointer(app, InputEvent::PointerMove { x: sx + 16., y: sy + 8. });
    for step in 1..=8 {
        let part = step as f32 / 8.;
        pointer(app, InputEvent::PointerMove { x: sx + (x - sx) * part, y: sy + (y - sy) * part });
    }
    assert!(app.ui.input.drag_state().is_some(), "a drag under way");
    app.frames(4);
    during(app);
    pointer(app, InputEvent::PointerUp { x, y, button });
    app.ui.input.forget_clicks();
    app.frames(20);
}

/// Where the sidebar's row `title` is grabbed.
fn row_at(app: &mut TestApp, title: &str) -> (f32, f32) {
    let row = app.bounds(&format!("#shell-sidebar >> role:button \"{title}\""));
    (row.x + 40., row.y + row.height / 2.)
}

/// The sidebar's row `title` dragged to `(x, y)` in the window.
fn drag_row_to(app: &mut TestApp, title: &str, x: f32, y: f32) {
    let from = row_at(app, title);
    carry(app, from, (x, y), |_| {});
}

/// What the preview shows while a chat is held: a split's landing (the
/// half it would take), or the chat page's "Drop to mention".
fn showing(app: &mut TestApp) -> (bool, bool) {
    (app.find("split-landing").is_some(), app.find("#chat-drop-mention").is_some())
}

fn mentioned(app: &mut TestApp, title: &str) -> bool {
    app.find(&format!("#chat-composer >> \"{title}\"")).is_some()
}

fn middle(r: Rect) -> (f32, f32) {
    (r.x + r.width / 2., r.y + r.height / 2.)
}

/// Chat `title` dropped on the right of the chat showing, its whole page
/// being that chat's.
fn split_right(app: &mut TestApp, title: &str) {
    let page = app.bounds("shell-page");
    drag_row_to(app, title, page.x + page.width * 0.92, page.y + page.height / 2.);
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.
}

#[test]
fn a_chat_dragged_to_a_chat_s_edge_sits_beside_it_and_is_focused() {
    let mut app = mount(&scratch("drag"));
    open(&mut app, "Fix the build");
    let alone = app.bounds("shell-page");
    assert_eq!(panes(&mut app), 0, "a chat alone is no split");
    split_right(&mut app, "Tidy the tests");
    assert_eq!(panes(&mut app), 2, "{}", app.tree());
    // The page's card stands over the focused chat, the one dropped: the
    // right half of what the chat had alone, the window's gap between.
    let page = app.bounds("shell-page");
    let gap = 4.;
    assert!(near(page.width, (alone.width - gap) / 2.), "{page:?} in {alone:?}");
    assert!(near(page.x + page.width, alone.x + alone.width), "{page:?} in {alone:?}");
    assert!(near(page.height, alone.height));
}

#[test]
fn command_w_closes_the_right_panel_tab_before_the_focused_split_chat() {
    let mut app = mount(&scratch("close-tab-before-chat"));
    open(&mut app, "Fix the build");
    split_right(&mut app, "Tidy the tests");
    assert_eq!(panes(&mut app), 2);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    app.frames(30);
    assert!(app.find("#panel-strip >> role:tab").is_some());
    app.click("#chat-composer >> role:textbox");
    app.frames(10);
    assert!(app.host.run_command("shell.close"));
    app.frames(30);
    assert!(app.find("#panel-strip >> role:tab").is_none(), "the panel tab closed");
    assert_eq!(panes(&mut app), 2, "both split chats remain open");
    // Once the panel has no tab, the same command closes the focused split.
    assert!(app.host.run_command("shell.close"));
    app.frames(30);
    assert_eq!(panes(&mut app), 0, "one chat remains without split headers");
}

#[test]
fn a_click_focuses_a_pane_and_its_close_takes_it_out() {
    let mut app = mount(&scratch("focus"));
    open(&mut app, "Fix the build");
    let alone = app.bounds("shell-page");
    split_right(&mut app, "Tidy the tests");
    let right = app.bounds("shell-page");
    // A click in the left pane: it's focused, the page's card moves to it.
    let button = PointerButton::Primary;
    let (x, y) = (alone.x + 60., alone.y + 8.);
    pointer(&mut app, InputEvent::PointerMove { x, y });
    pointer(&mut app, InputEvent::PointerDown { x, y, button });
    pointer(&mut app, InputEvent::PointerUp { x, y, button });
    app.ui.input.forget_clicks();
    app.frames(20);
    let left = app.bounds("shell-page");
    assert!(near(left.x, alone.x) && near(left.width, right.width), "{left:?}");
    // Closed (⌘W, the focused one): back to one chat, its page the whole of it
    // again.
    assert!(app.host.run_command("shell.close"));
    app.frames(30);
    assert_eq!(panes(&mut app), 0);
    let after = app.bounds("shell-page");
    assert!(near(after.x, alone.x) && near(after.width, alone.width), "{after:?} {alone:?}");
}

#[test]
fn a_split_is_kept_across_a_reload_and_a_launch() {
    let storage = scratch("kept");
    {
        let mut app = mount(&storage);
        open(&mut app, "Fix the build");
        split_right(&mut app, "Tidy the tests");
        assert_eq!(panes(&mut app), 2);
        app.host.reload().expect("the reload failed");
        app.frames(30);
        assert_eq!(panes(&mut app), 2, "after a reload");
        // (Saved as the host closes, as the app's is.)
        app.host.save();
    }
    let mut app = mount(&storage);
    open(&mut app, "Fix the build");
    assert_eq!(panes(&mut app), 2, "opening one of its chats again brings the split back");
    open(&mut app, "Write the docs");
    assert_eq!(panes(&mut app), 0, "a chat in no split shows alone");
}

/// The panes are placed by edges that are each a share of the page plus
/// points, so resizing the window lays them out with no script of the
/// split's own: what runs is each pane's content following its width (its
/// transcript's measure, its composer's). So a resize's script grows with
/// the panes, each one more costing about what one pane does, never
/// faster. (A chat alone is the chat page, its fixture's transcript empty:
/// told, not compared.)
#[test]
fn resizing_split_panes_costs_each_about_what_one_chat_does() {
    // (The sidebar folded, so it can't step aside mid-resize and be what's counted;
    // and each pane about as wide, about 630, under the conversation's width all
    // the way, so each follows the window and what's compared is how many. A split
    // set as it's kept: its rows are out of reach.)
    let mut calls = Vec::new();
    for (n, ids) in [&["a1"][..], &["a1", "a3"], &["a1", "a3", "a2"], &["a1", "a3", "a2", "b1"]].into_iter().enumerate() {
        let wide = 40. + 630. * ids.len().max(2) as f32;
        let mut app = mount(&scratch(&format!("resize-{n}")));
        app.ui.scene.borrow_mut().resize(wide, H);
        app.frames(10);
        split_of(&mut app, ids);
        open(&mut app, "Fix the build");
        assert!(app.host.run_command("shell.toggle_sidebar"));
        app.frames(60);
        assert_eq!(panes(&mut app), if n == 0 { 0 } else { n + 1 });
        let before = app.calls();
        for w in (0..12).map(|i| wide - 20. * i as f32) {
            app.ui.scene.borrow_mut().resize(w, H);
            app.frame();
        }
        app.ui.scene.borrow_mut().resize(wide, H);
        app.frames(5);
        calls.push(app.calls() - before);
    }
    eprintln!("resize script calls, 1 to 4 chats: {calls:?}");
    let pane = calls[1] as f64 / 2.;
    for (n, c) in calls.iter().enumerate().skip(2) {
        let per = *c as f64 / (n + 1) as f64;
        assert!(per <= pane * 1.25, "{} chats cost {c} script calls in a resize ({per:.0} each), two {pane:.0} each: {calls:?}", n + 1);
    }
    let steps: Vec<i64> = calls.windows(2).skip(1).map(|w| w[1] as i64 - w[0] as i64).collect();
    assert!(steps.iter().all(|s| (*s as f64) <= pane * 1.25), "each pane more costs about one pane: {calls:?}");
}

/// A chat carried from the sidebar: over a pane's edge it's a split (the
/// half it would take showing), over the pane's middle or its composer
/// it's mentioned ("Drop to mention" showing); each drop does what showed.
#[test]
fn a_sidebar_row_splits_at_an_edge_and_attaches_in_the_middle_or_the_composer() {
    let mut app = mount(&scratch("row-targets"));
    open(&mut app, "Fix the build");
    let page = app.bounds("shell-page");
    // The middle: attach.
    let from = row_at(&mut app, "Write the docs");
    carry(&mut app, from, middle(page), |app| assert_eq!(showing(app), (false, true), "the middle shows Drop to mention"));
    assert_eq!(panes(&mut app), 0, "the middle doesn't split");
    assert!(mentioned(&mut app, "Write the docs"), "the middle attaches:\n{}", app.tree());
    // The composer, low in the page between the side bands: attach.
    let composer = middle(app.bounds("#chat-composer"));
    let from = row_at(&mut app, "Ship the release");
    carry(&mut app, from, composer, |app| assert_eq!(showing(app), (false, true), "the composer shows Drop to mention"));
    assert_eq!(panes(&mut app), 0, "the composer doesn't split");
    assert!(mentioned(&mut app, "Ship the release"), "the composer attaches");
    // The left edge, and the top: a split, its landing showing.
    let from = row_at(&mut app, "Tidy the tests");
    carry(&mut app, from, (page.x + 20., page.y + page.height / 2.), |app| {
        assert_eq!(showing(app), (true, false), "the edge shows where it lands")
    });
    assert_eq!(panes(&mut app), 2, "the edge splits");
    let right = app.bounds("shell-page");
    assert!(near(right.x, page.x), "dropped at the left, it's the left one, focused: {right:?}");
    assert!(!mentioned(&mut app, "Tidy the tests"), "an edge doesn't attach");
}

/// Where pane `id`'s grip is grabbed: at its top's middle, there once the pointer
/// is near.
fn grip_of(app: &mut TestApp, id: &str) -> (f32, f32) {
    let b = app.bounds(&format!("#split-pane-{id}"));
    pointer(app, InputEvent::PointerMove { x: b.x + b.width / 2., y: b.y + 10. });
    app.frames(4);
    middle(app.bounds(&format!("#split-pane-{id} >> .pane-grip")))
}

/// A pane's grip carried over another pane: its edge moves the pane there,
/// its middle and its composer mention it there.
#[test]
fn a_pane_s_grip_moves_it_at_an_edge_and_attaches_in_the_middle_or_the_composer() {
    let mut app = mount(&scratch("grip-targets"));
    open(&mut app, "Fix the build");
    split_right(&mut app, "Tidy the tests");
    // The left pane's grip (Fix the build). (A press in a pane focuses it, the
    // grip's too: the right one, not focused then, has "Drop to mention" of its
    // own; dropped there, focused and mentioned.)
    let right = app.bounds("shell-page");
    // Its composer, at rest.
    let composer = middle(app.bounds("#chat-composer"));
    let grip = grip_of(&mut app, "a1");
    carry(&mut app, grip, composer, |app| assert!(app.find("#split-drop-mention-a3").is_some(), "its composer shows Drop to mention"));
    assert_eq!(panes(&mut app), 2, "still the two");
    assert!(mentioned(&mut app, "Fix the build"), "its composer attaches:\n{}", app.tree());
    // Its middle: mentioned. (A chat is mentioned once: the same chip.)
    let grip = grip_of(&mut app, "a1");
    carry(&mut app, grip, middle(right), |app| assert!(app.find("#split-drop-mention-a3").is_some(), "its middle shows Drop to mention"));
    assert_eq!(panes(&mut app), 2, "still the two");
    assert!(mentioned(&mut app, "Fix the build"), "its middle attaches");
    // Its bottom band (above its composer, between the side bands): the
    // pane moves under it, the two stacked.
    let composer = app.bounds("#chat-composer");
    let low = (right.x + right.width / 2., composer.y - 20.);
    let grip = grip_of(&mut app, "a1");
    carry(&mut app, grip, low, |app| assert_eq!(showing(app), (true, false), "the bottom band shows where it lands"));
    assert_eq!(panes(&mut app), 2);
    let now = app.bounds("shell-page");
    assert!(now.width > right.width * 1.5 && now.height < right.height * 0.6, "stacked: {now:?} from {right:?}");
}

/// A pane's composer grown past its rest (a chip in its draft) is still its
/// composer to a chat carried over it, focused or not: mentioned, not a split.
#[test]
fn a_pane_s_grown_composer_takes_a_drop_as_a_mention() {
    let mut app = mount(&scratch("grown-composer"));
    open(&mut app, "Fix the build");
    split_right(&mut app, "Tidy the tests");
    let right = app.bounds("shell-page");
    // A chip in the right pane's draft.
    let grip = grip_of(&mut app, "a1");
    carry(&mut app, grip, middle(right), |_| {});
    assert!(mentioned(&mut app, "Fix the build"));
    let composer = middle(app.bounds("#chat-composer"));
    // Over it, the right pane not focused (the grip's press focused the left).
    let grip = grip_of(&mut app, "a1");
    carry(&mut app, grip, composer, |app| assert_eq!(showing(app).0, false, "its composer shows where a split lands"));
    assert!(near(app.bounds("shell-page").x, app.bounds("#split-pane-a3").x), "the pane stayed where it was");
}

// ── Zeron v2 ────────────────────────────────────────────────────────────────

/// The window going to the back closes the composer's menu, as every popover; it
/// isn't dismissed, so back at the front it opens again. Escape does dismiss it.
#[test]
fn the_composers_menu_comes_back_with_the_window_and_stays_shut_after_escape() {
    let mut app = mount(&scratch("menu-window"));
    open(&mut app, "Fix the build");
    // A window so short its menu shows past it: those are what the window's going to
    // the back closes.
    app.ui.set_popup_screens(vec![zgx::scene::Rect::new(-4000., -4000., 8000., 8000.)]);
    app.ui.scene.borrow_mut().resize(1200., 260.);
    app.ui.set_window_active(true);
    app.host.window_state(&app.ui);
    app.frames(20);
    let field = "#chat-composer-dock >> role:textbox \"Message\"";
    app.click(field);
    app.type_text(field, "@");
    app.frames(20);
    let open_now = |app: &mut TestApp| {
        app.tree().lines().any(|l| l.contains("#mention-panel") && !l.contains(" hidden"))
    };
    assert!(open_now(&mut app), "typed: its menu\n{}", app.tree());
    // Either order of the two, as the platform has them.
    for popups_first in [true, false] {
        if popups_first {
            app.ui.dismiss_popups();
            app.frames(2);
            app.ui.set_window_active(false);
        } else {
            app.ui.set_window_active(false);
            app.frames(2);
            app.ui.dismiss_popups();
        }
        app.frames(20);
        assert!(!open_now(&mut app), "at the back: closed");
        app.ui.set_window_active(true);
        app.frames(20);
        assert!(open_now(&mut app), "back at the front: open again (popups first: {popups_first})");
    }
    app.keys("Escape");
    app.frames(20);
    assert!(!open_now(&mut app), "Escape: closed");
    // (A while later: a popover closing just as the window leaves counts as its doing.)
    std::thread::sleep(std::time::Duration::from_millis(600));
    app.ui.dismiss_popups();
    app.ui.set_window_active(false);
    app.frames(20);
    app.ui.set_window_active(true);
    app.frames(20);
    assert!(!open_now(&mut app), "dismissed stays dismissed");
}

/// Leaving Settings, the composer is where it was, no glide: the page hidden behind
/// them, with no height, isn't somewhere it moved to.
#[test]
fn coming_back_from_settings_the_composer_doesnt_move() {
    let mut app = mount(&scratch("settings-composer"));
    app.ui.scene.borrow_mut().resize(1400., H);
    app.frames(60);
    let home = app.bounds("#chat-composer").y;
    assert!(app.host.run_command("shell.settings"));
    // (In real time: what's hidden settling while Settings show.)
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(16));
        app.frame();
    }
    assert!(app.host.run_command("shell.new_chat"));
    let mut seen = Vec::new();
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(16));
        app.frame();
        seen.push(app.bounds("#chat-composer").y);
    }
    for y in &seen {
        assert!((y - home).abs() <= 1., "at {home}, then {seen:?}");
    }
    // Cmd+R: the panel, here too.
    assert!(app.host.run_command("shell.toggle_files"));
    app.frames(60);
    assert!(app.bounds("#shell-files").width > 300., "the panel opens on a new chat");
}

/// A templates folder as a launch leaves it (the default's copy, and the
/// official templates installed beside it), with `.live` composed for
/// `id` and the shell's fixture (and what it loads, beside app.luau) in it.
fn compose(t: &Path, id: &str) -> PathBuf {
    live::compose(t, id, &Default::default(), false).unwrap();
    let at = t.join(live::LIVE);
    for entry in std::fs::read_dir(ui()).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".luau") && !at.join(&name).exists() {
            std::fs::copy(entry.path(), at.join(&name)).unwrap();
        }
    }
    at
}

#[test]
fn zeron_v2_is_installed_as_zeron_s_and_runs_one_page() {
    let root = scratch("v2");
    let t = root.join("templates");
    live::sync_zeron(&ui(), &t).unwrap();
    // Installed beside the default, listed second, built in, trusted.
    let listed = live::installed(&t);
    let ids: Vec<&str> = listed.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(&ids[..2], [live::BUILTIN, "zeron-v2"]);
    assert_eq!((listed[0].name.as_str(), listed[1].name.as_str()), ("Zeron v3", "Zeron v2"));
    assert_eq!(listed[1].description, "The single-page layout from before Zeron v3.");
    assert!(listed[1].official.is_some() && !listed[1].out_of_date);
    assert!(preview::trusted_at(&t, "zeron-v2", &root.join("approvals.json")), "no consent asked of Zeron's own");
    assert!(official::verified(&t.join("zeron-v2")).is_some());

    // v3, the default: the page a card under the top bar.
    let live_dir = compose(&t, live::BUILTIN);
    let storage = root.join("state");
    let mut app = mount_at(&live_dir.join("shell_fixture.luau"), &storage);
    open(&mut app, "Fix the build");
    let v3 = app.bounds("shell-page");
    assert!(near(v3.y, 42.), "v3's page is a card under the top bar: {v3:?}");

    // Switched to v2, as a switch does: `.live` composed again, the window
    // reloaded in place. One page, to the window's edges: no card, and a
    // chat dragged to its edge doesn't split it.
    compose(&t, "zeron-v2");
    app.host.reload().expect("v2 didn't load");
    app.frames(30);
    let v2 = app.bounds("shell-page");
    assert!(near(v2.y, 0.) && v2.width > v3.width, "v2's page fills its room: {v2:?} (v3's {v3:?})");
    split_right(&mut app, "Tidy the tests");
    assert_eq!(panes(&mut app), 0, "v2 has no split chats");
    assert!(app.error().is_none(), "{:?}", app.error());

    // And back to v3.
    compose(&t, live::BUILTIN);
    app.host.reload().expect("v3 didn't load again");
    app.frames(30);
    open(&mut app, "Fix the build");
    assert!(near(app.bounds("shell-page").y, 42.));
}

// ── Drop indicators stand over the composers ────────────────────────────────

/// Whether `over` is in front of `under` at `(x, y)`: both under the point,
/// `over` first in the scene's front-to-back order.
fn in_front(app: &mut TestApp, over: &str, under: &str, (x, y): (f32, f32)) -> Result<(), String> {
    let a = app.target(over)?;
    let b = app.target(under)?;
    let hits = app.ui.scene.borrow().hit_test_all(x, y);
    let (ia, ib) = (hits.iter().position(|h| *h == a), hits.iter().position(|h| *h == b));
    match (ia, ib) {
        (Some(ia), Some(ib)) if ia < ib => Ok(()),
        _ => Err(format!("{over} at {ia:?}, {under} at {ib:?}, of {} under ({x}, {y})", hits.len())),
    }
}

/// Each drop indicator, held, draws in front of the composer it covers:
/// a split's landing over the focused pane's composer (the window's) and
/// over another's (its own), and "Drop to mention" over both.
#[test]
fn every_drop_indicator_stands_over_the_composer_it_covers() {
    let mut app = mount(&scratch("indicators"));
    open(&mut app, "Fix the build");
    split_right(&mut app, "Tidy the tests");
    // (a3 focused, at the right; a1 at the left.)
    let composer = middle(app.bounds("#chat-composer"));
    let left_composer = middle(app.bounds("#lane-composer-split-a1"));
    let left = app.bounds("#split-pane-a1");
    let right = app.bounds("#split-pane-a3");
    // A split's landing, the bottom half: over the window's composer, and
    // over the left pane's own.
    let from = row_at(&mut app, "Write the docs");
    let low = (right.x + right.width / 2., composer.1 - 110.);
    carry(&mut app, from, low, |app| {
        assert_eq!(showing(app), (true, false));
        in_front(app, "split-landing", "#chat-composer", composer).unwrap();
    });
    // (Dropped: a2 under a3. Back to two: a2 out.)
    app.host.run_command("shell.close");
    app.frames(20);
    let from = row_at(&mut app, "Write the docs");
    let left_low = (left.x + left.width / 2., left_composer.1 - 110.);
    carry(&mut app, from, left_low, |app| {
        in_front(app, "split-landing", "#lane-composer-split-a1", left_composer).unwrap();
    });
    app.host.run_command("shell.close");
    app.frames(20);
    // "Drop to mention": over the focused pane's middle (the window's
    // composer under it), and over the other's.
    open(&mut app, "Tidy the tests");
    let right = app.bounds("#split-pane-a3");
    let composer = middle(app.bounds("#chat-composer"));
    let from = row_at(&mut app, "Answer the review");
    carry(&mut app, from, middle(right), |app| {
        assert_eq!(showing(app), (false, true));
        in_front(app, "#chat-drop-mention", "#chat-composer", composer).unwrap();
    });
    let left = app.bounds("#split-pane-a1");
    let left_composer = middle(app.bounds("#lane-composer-split-a1"));
    let from = row_at(&mut app, "Answer the review");
    carry(&mut app, from, (left.x + left.width / 2., left.y + left.height * 0.45), |app| {
        in_front(app, "#split-drop-mention-a1", "#lane-composer-split-a1", left_composer).unwrap();
    });
}

// ── Focus: one source of truth ──────────────────────────────────────────────

/// The host's chats, as the fixture plays them (`fixture.chats`): one record
/// a chat, shown by every view watching it, the window's open chat and each
/// lane, as the engine's are.
#[derive(Default)]
struct Host {
    chats: std::collections::BTreeMap<String, Record>,
}

#[derive(Default, Clone)]
struct Record {
    reply: Option<String>,
    tools: usize,
    running: bool,
    asking: bool,
}

impl Host {
    fn publish(&self, app: &TestApp) {
        use zgx_script::Value;
        let s = |v: &str| Value::from(v);
        let chats = self.chats.iter().map(|(id, r)| {
            let marker = Value::record([("id", s(&format!("{id}-m"))), ("role", s("assistant")), ("text", s(&format!("This is {id}")))]);
            let mut messages = vec![marker];
            let live = format!("live-{id}");
            if r.reply.is_some() || r.tools > 0 {
                let tools: Vec<Value> = (1..=r.tools)
                    .map(|n| Value::record([("id", s(&format!("t{n}"))), ("kind", s("exec")), ("verb", s("Ran")), ("target", s(&format!("tool {id} {n}")))]))
                    .collect();
                messages.push(Value::record([
                    ("id", s(&live)),
                    ("role", s("assistant")),
                    ("text", s("")),
                    ("streaming", Value::Bool(true)),
                    ("tools", Value::List(std::rc::Rc::from(tools))),
                ]));
            }
            let mut session = vec![
                ("id", s(id)),
                ("state", s("ready")),
                ("running", Value::Bool(r.running)),
                ("harness", s("claude-code")),
                ("model", s("claude/opus")),
                ("effort", s("medium")),
                ("access", s("full")),
                ("project", s("atlas")),
                ("device", s("mac")),
            ];
            if r.asking {
                let option = |v: &str| Value::record([("value", s(v)), ("label", s(v))]);
                let question = Value::record([
                    ("header", s("Question")),
                    ("text", s(&format!("Which way for {id}?"))),
                    ("options", Value::List(std::rc::Rc::from(vec![option("left"), option("right")]))),
                ]);
                session.push(("ask", Value::record([("request", s(&format!("q-{id}"))), ("kind", s("question")), ("question", question)])));
            }
            let draft = Value::record([("id", s(&live)), ("text", s(r.reply.as_deref().unwrap_or("")))]);
            let record = Value::record([
                ("messages", Value::List(std::rc::Rc::from(messages))),
                ("draft", draft),
                ("session", Value::record(session)),
            ]);
            (id.clone(), record)
        });
        app.host.shared("fixture.chats", Value::Nil).set(Value::record(chats.collect::<Vec<_>>()));
        // (What leaves, a question answered, goes as it eases out.)
        app.frames(30);
    }
    fn chat(&mut self, id: &str) -> &mut Record {
        self.chats.entry(id.to_string()).or_default()
    }
}

/// A reply streaming into chat `id`: its words so far.
fn stream(app: &TestApp, host: &mut Host, id: &str, text: &str) {
    host.chat(id).reply = Some(text.to_string());
    host.publish(app);
}

/// Chat `id` running a turn, or not.
fn run(app: &TestApp, host: &mut Host, id: &str, running: bool) {
    host.chat(id).running = running;
    host.publish(app);
}

fn route_session(app: &TestApp) -> Option<String> {
    let path = app.host.states().into_iter().find(|(n, _)| n.ends_with("shell_route_path")).map(|(_, v)| match v {
        zgx_script::Value::Str(s) => s.to_string(),
        _ => String::new(),
    })?;
    path.strip_prefix("/session/").map(str::to_owned)
}

/// A textbox's value, from its line in the tree.
fn value_of(app: &mut TestApp, target: &str) -> Option<String> {
    let node = app.target(target).ok()?;
    let opts = zgx_script::driver::TreeOptions { depth: Some(0), budget: 2, all: false };
    let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, Some(node), &opts);
    let line = zgx_script::driver::tree_lines_text(&lines, hidden);
    let (_, rest) = line.split_once(" =\"")?;
    Some(rest.rsplit_once('"')?.0.to_owned())
}

fn has(app: &mut TestApp, target: &str) -> bool {
    app.find(target).is_some()
}

/// The panes showing, by session id, left to right then top to bottom.
fn shown_panes(app: &mut TestApp) -> Vec<String> {
    let tree = full_tree(app);
    let mut ids: Vec<(f32, f32, String)> = tree
        .lines()
        .filter_map(|l| {
            let id = l.split_whitespace().find_map(|w| w.strip_prefix("#split-pane-"))?.to_owned();
            let at = l.split_whitespace().find(|w| w.contains(',') && w.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-'))?;
            let (x, y) = at.split_once(',')?;
            Some((y.parse().ok()?, x.parse().ok()?, id))
        })
        .collect();
    ids.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // (A chat alone has its place kept, with nothing of a pane showing.)
    if ids.len() < 2 {
        return Vec::new();
    }
    ids.into_iter().map(|(_, _, id)| id).collect()
}

/// What the test expects each chat to show, besides what the host has
/// (`Host`): its title and its draft.
#[derive(Default, Clone)]
struct Chat {
    title: String,
    draft: String,
}

/// The composer that stands at chat `id`'s foot: the window's over the
/// focused pane, its own over another.
fn composer_of(id: &str, focused: bool) -> String {
    if focused { "#chat-composer-dock".to_string() } else { format!("#lane-composer-split-{id}") }
}

/// What chat `id` shows in its pane, focused or not, as `Host` and `Chat`
/// say: its transcript (its own words, its reply, its tools), its status,
/// its question, its draft.
fn shows_its_own(app: &mut TestApp, host: &Host, c: &Chat, id: &str, scope: &str, focused: bool, when: &str) {
    let r = host.chats.get(id).cloned().unwrap_or_default();
    assert!(has(app, &format!("{scope} >> ~\"This is {id}\"")), "{when}: {id} shows its own transcript");
    if let Some(s) = &r.reply {
        assert!(has(app, &format!("{scope} >> ~\"{s}\"")), "{when}: {id} shows its reply as it streams: {s}");
    }
    if r.tools > 0 {
        // (Its tool calls, as a reply's run of them shows: "Ran 2 commands".)
        let n = r.tools;
        assert!(has(app, &format!("{scope} >> ~\"Ran {n} command\"")), "{when}: {id} shows its {n} tool calls");
    }
    let composer = composer_of(id, focused);
    let question = has(app, &format!("{composer} >> role:radiogroup \"Which way for {id}?\""));
    assert_eq!(question, r.asking, "{when}: {id}'s question shows ({})", r.asking);
    if !r.asking {
        let textbox = format!("{composer} >> role:textbox");
        assert_eq!(value_of(app, &textbox).unwrap_or_default(), c.draft, "{when}: {id}'s own draft");
        // (Running: Stop, or Queue message with words in the draft. Not:
        // Send message.)
        let running = has(app, &format!("{composer} >> role:button \"Stop\"")) || has(app, &format!("{composer} >> role:button \"Queue message\""));
        assert_eq!(running, r.running, "{when}: {id} shows its own status (running {})", r.running);
    }
}

/// Every pane shows its own chat, live, focused or not, and one pane is
/// focused: the route's. A chat alone shows its draft in the chat page.
fn check(app: &mut TestApp, host: &Host, chats: &std::collections::BTreeMap<String, Chat>, when: &str) {
    let route = route_session(app);
    let panes = shown_panes(app);
    if panes.is_empty() {
        if let Some(r) = &route
            && !host.chats.get(r).is_some_and(|c| c.asking)
        {
            let c = &chats[r];
            assert_eq!(value_of(app, "#chat-composer >> role:textbox").unwrap_or_default(), c.draft, "{when}: {r} alone, its draft");
        }
        return;
    }
    let route = route.unwrap_or_else(|| panic!("{when}: a split showing off a session"));
    assert!(panes.contains(&route), "{when}: the route {route} isn't one of the panes {panes:?}");
    let page = app.bounds("shell-page");
    let mut focused = Vec::new();
    for id in &panes {
        let c = &chats[id];
        let pane = format!("#split-pane-{id}");
        let b = app.bounds(&pane);
        if near(b.x, page.x) && near(b.y, page.y) && near(b.width, page.width) && near(b.height, page.height) {
            focused.push(id.clone());
        }
        for other in panes.iter().filter(|o| *o != id) {
            assert!(!has(app, &format!("{pane} >> ~\"This is {other}\"")), "{when}: {id} shows {other}'s transcript");
        }
        shows_its_own(app, host, c, id, &pane, *id == route, when);
        // Following its end as it streams, focused or not.
        let at_end = app
            .host
            .states()
            .into_iter()
            .find(|(n, _)| n.ends_with(&format!("transcript_at_end.lane.split-{id}")))
            .map(|(_, v)| matches!(v, zgx_script::Value::Bool(true)));
        assert_ne!(at_end, Some(false), "{when}: {id} follows its end");
    }
    assert_eq!(focused, vec![route.clone()], "{when}: exactly one pane focused, the route's ({panes:?})");
    // The tab says the chat focused.
    let title = &chats[&route].title;
    assert!(has(app, &format!("#session-tabs >> role:tab \"{title}\"")), "{when}: the tab showing says {title}");
}

fn split_into(app: &mut TestApp, title: &str, side: &str) {
    let page = app.bounds("shell-page");
    let to = match side {
        "left" => (page.x + 20., page.y + page.height * 0.4),
        "right" => (page.x + page.width - 20., page.y + page.height * 0.4),
        _ => (page.x + page.width / 2., page.y + 40.),
    };
    drag_row_to(app, title, to.0, to.1);
}

fn focus_pane(app: &mut TestApp, id: &str) {
    // (Off its grip, at its top's middle: anywhere else in it.)
    let b = app.bounds(&format!("#split-pane-{id}"));
    let (x, y) = (b.x + 40., b.y + 17.);
    let button = PointerButton::Primary;
    pointer(app, InputEvent::PointerMove { x, y });
    pointer(app, InputEvent::PointerDown { x, y, button });
    pointer(app, InputEvent::PointerUp { x, y, button });
    app.ui.input.forget_clicks();
    app.frames(10);
}

const TITLES: [(&str, &str); 5] =
    [("a1", "Fix the build"), ("a2", "Write the docs"), ("a3", "Tidy the tests"), ("b1", "Ship the release"), ("b2", "Answer the review")];

fn chats() -> std::collections::BTreeMap<String, Chat> {
    TITLES.iter().map(|(id, t)| (id.to_string(), Chat { title: t.to_string(), ..Default::default() })).collect()
}

/// A pane out of focus is as live as the one in focus, and gaining focus
/// changes nothing it shows: it streams and follows its end, shows a new
/// tool call, shows its turn running and the question its agent asks, and
/// keeps its draft, while another pane is focused; then focused, the same.
/// (Nothing slows a pane out of focus: each shows its own chat's every
/// update, as it comes.)
#[test]
fn a_pane_out_of_focus_is_live_and_gaining_focus_changes_nothing() {
    let mut app = mount(&scratch("live"));
    let mut host = Host::default();
    let mut c = chats();
    open(&mut app, "Fix the build");
    split_right(&mut app, "Tidy the tests");
    check(&mut app, &host, &c, "split");
    // a1 isn't focused (a3 is): its reply streams in, tool calls and all.
    for n in 1..=5 {
        stream(&app, &mut host, "a1", &format!("stream a1 {n}"));
        if n == 3 {
            host.chat("a1").tools = 1;
            host.publish(&app);
        }
        check(&mut app, &host, &c, &format!("a1 streaming {n}"));
    }
    run(&app, &mut host, "a1", true);
    check(&mut app, &host, &c, "a1 running");
    // A draft typed in the focused chat stays with it as the focus moves.
    app.fill("#chat-composer >> role:textbox", "for a3");
    app.frames(5);
    c.get_mut("a3").unwrap().draft = "for a3".into();
    check(&mut app, &host, &c, "a3's draft");
    // Focused: a1 shows the same, its draft the window's composer's now.
    focus_pane(&mut app, "a1");
    check(&mut app, &host, &c, "a1 focused");
    app.fill("#chat-composer >> role:textbox", "for a1");
    app.frames(5);
    c.get_mut("a1").unwrap().draft = "for a1".into();
    // a1's agent asks, while a3 is focused; a3 focused again, then a1.
    focus_pane(&mut app, "a3");
    host.chat("a1").asking = true;
    host.publish(&app);
    check(&mut app, &host, &c, "a1 asks, out of focus");
    focus_pane(&mut app, "a1");
    check(&mut app, &host, &c, "a1 asks, focused");
    host.chat("a1").asking = false;
    host.publish(&app);
    check(&mut app, &host, &c, "a1 answered");
    // The panel: the focused chat's, and in no pane.
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    assert!(app.bounds("#shell-files").width > 1., "a1's panel opened");
    for id in ["a1", "a3"] {
        assert!(!has(&mut app, &format!("#split-pane-{id} >> #panel")), "the panel isn't in {id}'s pane");
    }
    focus_pane(&mut app, "a3");
    app.frames(40);
    check(&mut app, &host, &c, "a3 focused, a1's panel");
    // Each pane's place is its chat's own: kept by its id.
    let places: Vec<String> = app.host.states().into_iter().map(|(n, _)| n).filter(|n| n.contains("chat_split_anchor_")).collect();
    assert!(places.iter().any(|n| n.ends_with("_a1")) && places.iter().any(|n| n.ends_with("_a3")), "{places:?}");
}

/// In a split, the side opened wins: the panel opened when the panes would go
/// under their least closes the sidebar, the sidebar opened closes the panel,
/// and neither is back until it's asked for. Under their least with one side
/// open, the panes just narrow. (ui/tests/pane_floor.test.luau has the rest,
/// which CI doesn't run.)
#[test]
fn the_side_opened_wins_in_a_split() {
    use zgx_script::Value;
    let row = |ids: &[&str]| {
        let leaf = |id: &str| Value::record([("kind", Value::from("leaf")), ("id", Value::from(id))]);
        let kids: Vec<Value> = ids.iter().map(|id| leaf(id)).collect();
        Value::List(std::rc::Rc::from(vec![Value::record([("kind", Value::from("row")), ("children", Value::List(std::rc::Rc::from(kids)))])]))
    };
    const PANEL: &str = "#titlebar >> role:button \"Panel\"";
    const SIDEBAR: &str = "#titlebar >> role:button \"Toggle sidebar\"";
    for (ids, least) in [(&["a1", "a2"][..], 288.), (&["a1", "a2", "a3"][..], 230.)] {
        let mut app = mount(&scratch(&format!("wins-{}", ids.len())));
        app.ui.scene.borrow_mut().resize(1100., H);
        app.frames(30);
        app.host.set_state("screens/shell/split.luau::inset_split_trees", row(ids)).unwrap();
        open(&mut app, "Fix the build");
        app.frames(40);
        let when = format!("{} panes at 1100", ids.len());
        let shown = |app: &mut TestApp, what: &str| app.bounds(what).width > 1.;
        app.click(PANEL);
        app.frames(60);
        assert_eq!(app.bounds("shell-files").width, 360., "{when}: the panel, at its least");
        assert!(!shown(&mut app, "shell-sidebar"), "{when}: the sidebar closed for it");
        for id in ids {
            let w = app.bounds(&format!("#split-pane-{id}")).width;
            assert!(w >= least, "{when}: {id} is {w} wide, beside the panel");
        }
        // The panel closed: the sidebar isn't back on its own.
        app.click(PANEL);
        app.frames(60);
        assert!(!shown(&mut app, "shell-sidebar"), "{when}: the sidebar stays closed");
        // Open both ways: the sidebar closes the panel, for good too.
        app.click(PANEL);
        app.frames(60);
        app.click(SIDEBAR);
        app.frames(60);
        assert!(shown(&mut app, "shell-sidebar"), "{when}: the sidebar opened");
        assert!(!shown(&mut app, "shell-files"), "{when}: the panel closed for it");
        app.click(SIDEBAR);
        app.frames(60);
        assert!(!shown(&mut app, "shell-files"), "{when}: the panel stays closed");
    }
}

/// A side closing for the other slides, and nothing steps with it: the tabs
/// easing past the bar's controls as the sidebar goes (the first pane's title,
/// in its head, once stood there at once, 164 points in a frame), each pane's
/// edges and the panel gliding, every frame.
#[test]
fn a_side_giving_way_steps_nothing() {
    use zgx_script::Value;
    let leaf = |id: &str| Value::record([("kind", Value::from("leaf")), ("id", Value::from(id))]);
    let kids = vec![leaf("a1"), leaf("a2")];
    let tree = Value::List(std::rc::Rc::from(vec![Value::record([("kind", Value::from("row")), ("children", Value::List(std::rc::Rc::from(kids)))])]));
    let mut app = mount(&scratch("no-steps"));
    app.ui.scene.borrow_mut().resize(1100., H);
    app.frames(30);
    app.host.set_state("screens/shell/split.luau::inset_split_trees", tree).unwrap();
    open(&mut app, "Fix the build");
    app.frames(60);
    // The most the columns' own glide moves anything in a frame here, with
    // room to spare; and the panel's own motion (ds/motion `panel`), which takes
    // 27% of its road in its first frame: its least, 360, here.
    const MOST: f32 = 80.;
    const PANEL_MOST: f32 = 100.;
    for (when, click) in [("the panel opened", "#titlebar >> role:button \"Panel\""), ("the sidebar opened", "#titlebar >> role:button \"Toggle sidebar\"")] {
        let sample = |app: &mut TestApp| {
            let (a1, a2) = (app.bounds("#split-pane-a1"), app.bounds("#split-pane-a2"));
            [app.bounds("#session-tabs-scroll").x, a1.x, a1.width, app.bounds("shell-files").x, a2.x + a2.width]
        };
        const NAMES: [&str; 5] = ["the tabs' start", "a1's left", "a1's width", "the panel's left", "a2's right"];
        let mut was = sample(&mut app);
        app.click(click);
        for f in 0..24 {
            let now = sample(&mut app);
            for i in 0..NAMES.len() {
                let most = if i >= 3 { PANEL_MOST } else { MOST };
                assert!((was[i] - now[i]).abs() <= most, "{when}, frame {f}: {} stepped from {} to {}", NAMES[i], was[i], now[i]);
            }
            was = now;
            app.frame();
        }
        app.frames(30);
    }
}

/// 200 steps, from a fixed seed: focus a pane, open a chat from the
/// sidebar, split one in by a drop, close a pane, type a draft in the
/// focused composer, stream a reply (and a tool call) into a chat that
/// isn't focused, change a chat's status, a chat's agent asking, reload,
/// resize; and in the focused chat's panel: open a tab from any source. After
/// each, every pane shows exactly its own chat, live as if focused (its
/// transcript, reply, tools, status, question and draft), and one pane is
/// focused: the route's, its tab saying its title; and each tab opened in the
/// panel showed, active.
#[test]
fn two_hundred_random_steps_keep_every_pane_its_own_chat() {
    // (Wide enough for four panes, the sidebar and the panel: fewer, and the
    // sidebar steps aside, its rows out of the steps' reach.)
    const WIDE: f32 = 2200.;
    let mut app = mount(&scratch("random"));
    app.ui.scene.borrow_mut().resize(WIDE, H);
    app.frames(30);
    let mut host = Host::default();
    let mut c = chats();
    let mut seed: u64 = 0x5eed_0f_2e0;
    let mut next = |n: usize| -> usize {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    open(&mut app, "Fix the build");
    let mut log: Vec<String> = Vec::new();
    for step in 0..200 {
        let panes = shown_panes(&mut app);
        let route = route_session(&app);
        let others: Vec<String> = panes.iter().filter(|p| Some((*p).clone()) != route).cloned().collect();
        let what = match next(14) {
            0 if panes.len() >= 2 => {
                let id = panes[next(panes.len())].clone();
                focus_pane(&mut app, &id);
                format!("focus {id}")
            }
            1 => {
                let (id, title) = TITLES[next(TITLES.len())];
                open(&mut app, title);
                format!("open {id}")
            }
            2 if route.is_some() && panes.len() < 4 => {
                let out: Vec<&(&str, &str)> = TITLES.iter().filter(|(id, _)| !panes.iter().any(|p| p == id) && Some(id.to_string()) != route).collect();
                if out.is_empty() {
                    continue;
                }
                let (id, title) = *out[next(out.len())];
                let side = ["left", "right", "top"][next(3)];
                split_into(&mut app, title, side);
                format!("split {id} {side}")
            }
            3 if panes.len() >= 2 => {
                app.host.run_command("shell.close");
                app.frames(20);
                format!("close {route:?}")
            }
            4 if route.is_some() && !host.chats.get(route.as_ref().unwrap()).is_some_and(|r| r.asking) => {
                let r = route.clone().unwrap();
                let text = format!("{r} draft {step}");
                // (The pointer off the sidebar row it last clicked: that row's
                // hover card can stand over the composer, as it can't for a
                // person, whose pointer leaves the row to get there.)
                app.hover("#shell-drag");
                app.frames(30);
                app.fill("#chat-composer >> role:textbox", &text);
                app.frames(4);
                c.get_mut(&r).unwrap().draft = text;
                format!("type in {r}")
            }
            5 if !others.is_empty() => {
                let id = others[next(others.len())].clone();
                stream(&app, &mut host, &id, &format!("stream {id} {step}"));
                if next(3) == 0 {
                    host.chat(&id).tools += 1;
                    host.publish(&app);
                }
                format!("stream into {id}")
            }
            6 if !panes.is_empty() => {
                let id = panes[next(panes.len())].clone();
                let on = !host.chats.get(&id).is_some_and(|r| r.running);
                run(&app, &mut host, &id, on);
                format!("{id} running {on}")
            }
            7 if !panes.is_empty() => {
                let id = panes[next(panes.len())].clone();
                let on = !host.chats.get(&id).is_some_and(|r| r.asking);
                host.chat(&id).asking = on;
                host.publish(&app);
                format!("{id} asking {on}")
            }
            8 => {
                app.host.reload().expect("the reload failed");
                app.frames(30);
                "reload".to_string()
            }
            10..=13 if route.is_some() => {
                let r = route.clone().unwrap();
                if app.bounds("#shell-files").width < 2. {
                    app.click("#titlebar >> role:button \"Panel\"");
                    app.frames(40);
                }
                let source = ["pr", "file", "url", "terminal"][next(4)];
                opens_a_tab(&mut app, &r, source, &format!("step {step}"));
                format!("panel {r}: open a {source}")
            }
            9 => {
                let w = 1100. + next(400) as f32;
                app.ui.scene.borrow_mut().resize(w, H);
                app.frames(6);
                app.ui.scene.borrow_mut().resize(WIDE, H);
                app.frames(10);
                format!("resize {w}")
            }
            _ => continue,
        };
        log.push(what.clone());
        let when = format!("step {step} ({what}; after {:?})", &log[log.len().saturating_sub(6)..]);
        check(&mut app, &host, &c, &when);
        assert!(app.error().is_none(), "{when}: {:?}", app.error());
    }
    assert!(log.len() >= 120, "most steps did something: {}", log.len());
    for kind in ["open a", "split a", "focus a", "close", "reload", "asking true", "stream into"] {
        let n = log.iter().filter(|w| w.contains(kind)).count();
        eprintln!("{kind}: {n}");
    }
    let panel = log.iter().filter(|w| w.starts_with("panel ")).count();
    assert!(panel >= 20, "the panel got its share: {panel} of {}", log.len());
}

// ── The panel's tabs ────────────────────────────────────────────────────────

/// A named state's value (`ui.state`s are named `module.luau::name`).
fn state_of(app: &TestApp, name: &str) -> zgx_script::Value {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n == name || n.ends_with(&format!("::{name}")))
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// Chat `chat`'s panel's active tab.
fn panel_active(app: &TestApp, chat: &str) -> Option<String> {
    let sessions = state_of(app, "panel_sessions");
    match sessions.field(&format!("chat:{chat}"))?.field("active")? {
        zgx_script::Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Opening a tab from source `source`: it's active, showing in the panel's
/// strip.
fn opens_a_tab(app: &mut TestApp, chat: &str, source: &str, when: &str) {
    assert!(app.host.run_command(&format!("fixture.open_{source}")), "{when}: fixture.open_{source}");
    app.frames(20);
    let active = panel_active(app, chat).unwrap_or_else(|| panic!("{when}: {source} opened no tab"));
    assert!(has(app, &format!("#panel-strip >> #panel-tab-{active}")), "{when}: the {source}'s tab shows in the strip");
}

// ── The panel's toggle over a chat of visuals ──────────────────────────────

/// The shell's fixture with the real host's visuals (src/visuals.rs).
fn mount_visuals(fixture: &Path, storage: &Path, w: f32) -> TestApp {
    let (fixture, storage) = (fixture.to_path_buf(), storage.to_path_buf());
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, fixture).storage(storage);
        crate::visuals::register(&host);
        host
    });
    app.ui.scene.borrow_mut().resize(w, 820.);
    app.frames(30);
    app
}

/// A visual's script from the visuals' fixture, by its name there.
fn visual_source(name: &str) -> String {
    let fixture = std::fs::read_to_string(ui().join("visuals_fixture.luau")).unwrap();
    let at = fixture.find(&format!("name = \"{name}\"")).unwrap();
    let from = at + fixture[at..].find("source = [[").unwrap() + "source = [[".len();
    let to = from + fixture[from..].find("]]").unwrap();
    fixture[from..to].to_string()
}

/// Chat `id` as the host has it: a reply of `blocks` (text, and visuals
/// when `visuals`), done.
fn reply_of_visuals(app: &TestApp, ids: &[(&str, bool)]) {
    use zgx_script::Value;
    let s = |v: &str| Value::from(v);
    let words = "A paragraph of the reply, long enough to wrap over a few lines at the chat's width, so its text lays out as a real reply's does, with more after it.";
    let chats: Vec<(String, Value)> = ids
        .iter()
        .map(|(id, visuals)| {
            let mut blocks = vec![Value::record([("id", s("b0")), ("kind", s("text")), ("text", s(words))])];
            for (n, name) in ["dashboard", "charts", "scene", "canvas"].iter().enumerate() {
                blocks.push(if *visuals {
                    Value::record([("id", s(&format!("v:{id}-{name}"))), ("kind", s("visual")), ("title", s(name)), ("source", s(&visual_source(name)))])
                } else {
                    Value::record([("id", s(&format!("t{n}"))), ("kind", s("text")), ("text", s(words))])
                });
                blocks.push(Value::record([("id", s(&format!("w{n}"))), ("kind", s("text")), ("text", s(words))]));
            }
            let messages = vec![
                Value::record([("id", s(&format!("{id}-q"))), ("role", s("user")), ("text", s("Show me the numbers."))]),
                Value::record([("id", s(&format!("{id}-r"))), ("role", s("assistant")), ("text", s("")), ("blocks", Value::List(std::rc::Rc::from(blocks)))]),
            ];
            let session = Value::record([("id", s(id)), ("state", s("ready")), ("running", Value::Bool(false))]);
            (id.to_string(), Value::record([("messages", Value::List(std::rc::Rc::from(messages))), ("session", session)]))
        })
        .collect();
    app.host.shared("fixture.chats", Value::Nil).set(Value::record(chats));
}

/// What a panel toggle costs, open and closed again: script calls, and the
/// frames' time (the most, and their sum), over its glide.
fn toggle_cost(app: &mut TestApp) -> (u64, f64, f64) {
    app.wait_until(|app| app.find("~\"Loading \"").is_none());
    app.frames(40);
    let calls = app.calls();
    let mut most: f64 = 0.;
    let mut all = 0.;
    let mut each: Vec<f64> = Vec::new();
    for _ in 0..2 {
        app.click("#titlebar >> role:button \"Panel\"");
        for _ in 0..30 {
            let t = std::time::Instant::now();
            app.frame();
            let ms = t.elapsed().as_secs_f64() * 1000.;
            most = most.max(ms);
            all += ms;
            each.push(ms);
        }
    }
    each.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!("MEASURE   median frame {:.2} ms, p90 {:.2} ms", each[each.len() / 2], each[each.len() * 9 / 10]);
    (app.calls() - calls, most, all)
}

#[test]
#[ignore = "a measurement: cargo test --bin zeron-zgx panel_toggle_over_visuals -- --ignored --nocapture"]
fn panel_toggle_over_visuals_measured() {
    let root = scratch("toggle-v2");
    let t = root.join("templates");
    live::sync_zeron(&ui(), &t).unwrap();
    let v2 = compose(&t, "zeron-v2").join("shell_fixture.luau");
    for (label, fixture) in [("v3", ui().join("shell_fixture.luau")), ("v2", v2)] {
        for (chat, visuals) in [("a1", false), ("a1", true)] {
            for w in [1100., 1500.] {
                let mut app = mount_visuals(&fixture, &scratch(&format!("toggle-{visuals}-{w}")), w);
                open(&mut app, "Fix the build");
                reply_of_visuals(&app, &[(chat, visuals)]);
                let (calls, most, all) = toggle_cost(&mut app);
                eprintln!("MEASURE {label} w{w} visuals={visuals}: {calls} script calls, frames {all:.1} ms in all, {most:.1} ms the most");
            }
        }
    }
    // A split of two, each its chat of visuals (v3).
    for visuals in [false, true] {
        let mut app = mount_visuals(&ui().join("shell_fixture.luau"), &scratch(&format!("toggle-split-{visuals}")), 1500.);
        open(&mut app, "Fix the build");
        split_right(&mut app, "Tidy the tests");
        reply_of_visuals(&app, &[("a1", visuals), ("a3", visuals)]);
        let (calls, most, all) = toggle_cost(&mut app);
        eprintln!("MEASURE v3 split w1500 visuals={visuals}: {calls} script calls, frames {all:.1} ms in all, {most:.1} ms the most");
    }
}



/// A panel toggle over a chat of visuals reflows the chat live, at every
/// frame of its glide, as v2 did: the transcript and a chart in it follow
/// the page's edge through an open and a close, never held at one width.
#[test]
fn a_panel_toggle_reflows_a_chat_of_visuals_live() {
    let toggle = |visuals: bool| -> (u64, std::collections::BTreeSet<i64>, std::collections::BTreeSet<i64>) {
        let mut app = mount_visuals(&ui().join("shell_fixture.luau"), &scratch(&format!("once-{visuals}")), 1100.);
        open(&mut app, "Fix the build");
        reply_of_visuals(&app, &[("a1", visuals)]);
        app.wait_until(|app| app.find("~\"Loading \"").is_none());
        app.frames(40);
        let calls = app.calls();
        let (mut transcript, mut chart) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
        for _ in 0..2 {
            app.click("#titlebar >> role:button \"Panel\"");
            for _ in 0..30 {
                app.frame();
                transcript.insert(app.bounds("#chat-session >> #transcript").width.round() as i64);
                if visuals {
                    let plot = app.target("#chat-session >> .chart-plot[1]");
                    if let Ok(node) = plot {
                        chart.insert(app.ui.scene.borrow().bounds(node).width.round() as i64);
                    }
                }
            }
        }
        (app.calls() - calls, transcript, chart)
    };
    let (_, plain_widths, _) = toggle(false);
    let (_, widths, chart) = toggle(true);
    assert!(plain_widths.len() >= 8 && widths.len() >= 8, "the transcript took {widths:?} (plain {plain_widths:?})");
    assert!(chart.len() >= 8, "a chart took {chart:?}");
}


// ── A new chat's page stays a card ──────────────────────────────────────────

/// The sidebar closing over a new chat's page: it stays a card, its fill and its
/// place under the top bar as they were, on every frame there and back.
#[test]
fn a_new_chat_s_page_stays_a_card_as_the_sidebar_folds() {
    let mut app = mount(&scratch("card"));
    app.frames(30);
    let sample = |app: &mut TestApp, what: &str| {
        for f in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(8));
            app.frame();
            let fill = app.opacity("shell-page-card");
            let top = app.bounds("shell-page").y;
            assert!((fill - 1.).abs() < 0.01, "{what}, frame {f}: the card's fill at {fill}");
            assert!(near(top, 42.), "{what}, frame {f}: the page's top at {top}");
        }
    };
    sample(&mut app, "at rest");
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    sample(&mut app, "folding");
    assert_eq!(app.bounds("shell-page").width, W - 10., "the window's edge each side of it");
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    sample(&mut app, "coming back");
}

// ── v2, Inset as uploaded, v3: what each costs ─────────────────────────────

/// The chats as the host has them, each a long conversation (twelve
/// exchanges of paragraphs), `streaming` one's reply growing.
fn long_chats(app: &TestApp, ids: &[&str], streaming: Option<(&str, usize)>) {
    use zgx_script::Value;
    let s = |v: &str| Value::from(v);
    let para = "The layout pass runs first and measures each row, then the paint pass draws what changed; a long reply wraps over several lines at the chat's width, as a real one does, with a list and some code after it.";
    let chats: Vec<(String, Value)> = ids
        .iter()
        .map(|id| {
            let mut messages = Vec::new();
            for n in 0..12 {
                messages.push(Value::record([("id", s(&format!("{id}-q{n}"))), ("role", s("user")), ("text", s("And then?"))]));
                messages.push(Value::record([("id", s(&format!("{id}-r{n}"))), ("role", s("assistant")), ("text", s(&format!("Reply {n} of {id}. {para}\n\n{para}\n\n- one\n- two\n\n```rust\nfn main() {{}}\n```")))]));
            }
            let mut draft = Value::record([("id", s("")), ("text", s(""))]);
            if let Some((sid, words)) = streaming
                && sid == *id
            {
                messages.push(Value::record([("id", s(&format!("{id}-live"))), ("role", s("assistant")), ("text", s("")), ("streaming", Value::Bool(true))]));
                let text: Vec<&str> = para.split(' ').cycle().take(words).collect();
                draft = Value::record([("id", s(&format!("{id}-live"))), ("text", s(&text.join(" ")))]);
            }
            let session = Value::record([("id", s(id)), ("state", s("ready")), ("running", Value::Bool(streaming.is_some_and(|(sid, _)| sid == *id)))]);
            (id.to_string(), Value::record([("messages", Value::List(std::rc::Rc::from(messages))), ("draft", draft), ("session", session)]))
        })
        .collect();
    app.host.shared("fixture.chats", Value::Nil).set(Value::record(chats));
}

/// Frames' work (ms): the median and the 95th percentile.
fn spread(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2], v[(v.len() * 95 / 100).min(v.len() - 1)])
}

fn timed(app: &mut TestApp) -> f64 {
    let t = std::time::Instant::now();
    app.frame();
    t.elapsed().as_secs_f64() * 1000.
}

/// A variant's costs, with `panes` chats side by side (1: alone).
fn costs(fixture: &Path, count: usize) -> String {
    const IDS: [&str; 6] = ["a1", "a2", "a3", "b1", "b2", "m1"];
    let mut app = mount_at(fixture, &scratch(&format!("perf-{count}")));
    app.ui.scene.borrow_mut().resize(1600., 1000.);
    assert!(app.host.run_command("many"));
    app.frames(30);
    let ids = &IDS[..count];
    long_chats(&app, &IDS, None);
    if count > 1 {
        use zgx_script::Value;
        let leaf = |id: &str| Value::record([("kind", Value::from("leaf")), ("id", Value::from(id))]);
        let kids: Vec<Value> = ids.iter().map(|id| leaf(id)).collect();
        let tree = Value::record([("kind", Value::from("row")), ("children", Value::List(std::rc::Rc::from(kids)))]);
        let set = app.host.set_state("screens/shell/split.luau::inset_split_trees", Value::List(std::rc::Rc::from(vec![tree])));
        assert!(set.is_ok(), "{set:?}");
    }
    open(&mut app, "Fix the build");
    app.frames(60);
    let shown = panes(&mut app).max(1);
    let opts = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: true };
    app.ui.prepare_frame();
    let nodes = zgx_script::driver::tree_lines(&app.ui, &app.handle, None, &opts).0.len();
    // Idle: nothing changing.
    let calls = app.calls();
    let idle: f64 = (0..120).map(|_| timed(&mut app)).sum();
    let idle_calls = app.calls() - calls;
    // A reply streaming in the focused chat, a few words a frame.
    let calls = app.calls();
    let mut stream = Vec::new();
    for n in 1..=60 {
        long_chats(&app, &IDS, Some(("a1", 4 * n)));
        stream.push(timed(&mut app));
    }
    let stream_calls = (app.calls() - calls) / 60;
    // The sidebar's edge dragged out and back, a move a frame.
    let edge = app.bounds("#shell-sidebar >> \"Resize sidebar\"");
    let (x, y) = (edge.x + edge.width / 2., edge.y + edge.height / 2.);
    let button = PointerButton::Primary;
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, Modifiers::default());
    let mut drag = Vec::new();
    for step in (1..=30).chain((0..30).rev()) {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + 4. * step as f32, y }, Modifiers::default());
        drag.push(timed(&mut app));
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, Modifiers::default());
    // (Let go: laid out once.)
    let settle = (0..5).map(|_| timed(&mut app)).fold(0., f64::max);
    let ((s50, s95), (d50, d95)) = (spread(stream), spread(drag));
    format!(
        "{shown} pane(s) | {nodes} nodes | idle 120 frames {idle:.2} ms, {idle_calls} script calls | stream p50 {s50:.2} p95 {s95:.2} ms, {stream_calls} calls/frame | sidebar drag p50 {d50:.2} p95 {d95:.2} ms, settle {settle:.2} ms"
    )
}

#[test]
#[ignore = "a measurement: cargo test --release --bin zeron-zgx costs_of_v2_inset_and_v3 -- --ignored --nocapture"]
fn costs_of_v2_inset_and_v3_measured() {
    let root = scratch("costs");
    let t = root.join("templates");
    // (`COSTS_UI`: another ui/ to compose, an earlier commit's; `COSTS_ONLY`:
    // one variant.)
    let from = std::env::var("COSTS_UI").map(PathBuf::from).unwrap_or_else(|_| ui());
    let only = std::env::var("COSTS_ONLY").ok();
    live::sync_zeron(&from, &t).unwrap();
    // Inset as uploaded, laid over today's default as the app lays it (its
    // shell/init.luau fails there on its missing scroll_probe and falls
    // back to the default's: left out).
    let pkg = std::env::var("INSET_PKG").expect("INSET_PKG: the decoded Inset package");
    for (rel, path) in live::files(Path::new(&pkg), |rel| rel.ends_with(".luau") && rel != "screens/shell/init.luau") {
        let to = t.join("inset").join(&rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(path, to).unwrap();
    }
    std::fs::write(t.join("inset/zeron-template.json"), r#"{"id":"inset","name":"Inset"}"#).unwrap();
    let mut rows = Vec::new();
    for (label, id, panes) in [("v2", "zeron-v2", vec![1]), ("Inset", "inset", vec![1, 2, 4, 6]), ("v3", live::BUILTIN, vec![1, 2, 4, 6])] {
        if only.as_deref().is_some_and(|o| o != label) {
            continue;
        }
        let at = root.join(format!("live-{label}"));
        let _ = std::fs::remove_dir_all(&at);
        let fixture = compose(&t, id);
        std::fs::rename(&fixture, &at).unwrap();
        for n in panes {
            rows.push(format!("{label}: {}", costs(&at.join("shell_fixture.luau"), n)));
            eprintln!("COST {}", rows.last().unwrap());
        }
    }
}

// ── Dragging a column's edge ───────────────────────────────────────────────

/// A text node's box: the first line of the tree under `scope` holding
/// `words`.
fn text_box(app: &mut TestApp, scope: &str, words: &str) -> Option<Rect> {
    let node = app.target(&format!("{scope} >> ~\"{words}\"")).ok()?;
    Some(app.ui.scene.borrow().bounds(node))
}

/// The sidebar's edge dragged `by` points, a move a frame, then let go:
/// `watch` read after each frame of it.
fn drag_sidebar<T>(app: &mut TestApp, by: f32, mut watch: impl FnMut(&mut TestApp) -> T) -> Vec<T> {
    let edge = app.bounds("#shell-sidebar >> \"Resize sidebar\"");
    let (x, y) = (edge.x + edge.width / 2., edge.y + edge.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    let mut seen = Vec::new();
    for step in 1..=20 {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + by * step as f32 / 20., y }, m);
        app.frame();
        seen.push(watch(app));
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x + by, y, button }, m);
    app.ui.input.forget_clicks();
    seen
}

/// A window with room for four panes, and the sidebar beside them dragged wider.
const DRAG_ROOM: f32 = 2000.;

/// Dragging the sidebar's edge, what's in the page reflows live: each
/// pane's transcript follows the edge at every move, never held at one
/// width; let go, the reading place is where it stood.
#[test]
fn a_dragged_edge_reflows_the_page_live_keeping_the_reading_place() {
    for panes in [1usize, 4] {
        const IDS: [&str; 4] = ["a1", "a2", "a3", "b1"];
        let mut app = mount(&scratch(&format!("edge-{panes}")));
        // (Room for the panes beside the sidebar: short of it, it steps aside,
        // and there's no edge to drag.)
        app.ui.scene.borrow_mut().resize(DRAG_ROOM, H);
        app.frames(10);
        long_chats(&app, &IDS, None);
        if panes > 1 {
            use zgx_script::Value;
            let kids: Vec<Value> = IDS[..panes].iter().map(|id| Value::record([("kind", Value::from("leaf")), ("id", Value::from(*id))])).collect();
            let tree = Value::record([("kind", Value::from("row")), ("children", Value::List(std::rc::Rc::from(kids)))]);
            app.host.set_state("screens/shell/split.luau::inset_split_trees", Value::List(std::rc::Rc::from(vec![tree]))).unwrap();
        }
        open(&mut app, "Fix the build");
        app.frames(30);
        let scope = if panes > 1 { "#split-pane-a1" } else { "#chat-session" };
        // Read from the middle of a1's conversation, settled there.
        let list = format!("{scope} >> #transcript");
        app.scroll(&list, -2500.);
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
        }
        // What's being read: the first text starting in view, as the
        // transcript takes it.
        let view = app.bounds(&list);
        let node = app.target(&list).unwrap();
        let opts = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: false };
        let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, Some(node), &opts);
        let reading = zgx_script::driver::tree_lines_text(&lines, hidden)
            .lines()
            .filter_map(|l| {
                let id = l.split_whitespace().next()?.to_owned();
                let (_, rest) = l.split_once("text \"")?;
                let (words, at) = rest.split_once("\" ")?;
                let y: f32 = at.split_whitespace().next()?.split(',').nth(1)?.parse().ok()?;
                (y >= view.y && words.len() > 12).then_some((y, id))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(_, id)| id)
            .expect("text in view");
        let before = app.bounds(&reading);
        let widths = drag_sidebar(&mut app, 120., |app| app.bounds(&list).width.round() as i64);
        let distinct: std::collections::BTreeSet<i64> = widths.iter().copied().collect();
        assert!(distinct.len() >= 15, "{panes} pane(s): the transcript followed the edge: {widths:?}");
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
        }
        let now = app.bounds(&reading);
        assert!((now.y - before.y).abs() <= 2., "{panes} pane(s): the reading place kept: {reading} from {} to {}", before.y, now.y);
    }
}

/// Dragging far enough for what's being read to rewrap, in panes under the
/// conversation's width: it stands still at every frame of the drag, not just
/// once the drag is over. (Its lines going past the reading place's own row
/// grow it upward, 24 points a rewrap, if the place falls to another row.)
#[test]
fn what_is_being_read_stands_still_at_every_frame_of_a_drag() {
    for (panes, room) in [(1usize, 1000f32), (2, 1400.), (4, 2000.)] {
        const IDS: [&str; 4] = ["a1", "a2", "a3", "b1"];
        let mut app = mount(&scratch(&format!("edge-frames-{panes}")));
        app.ui.scene.borrow_mut().resize(room, H);
        app.frames(10);
        long_chats(&app, &IDS, None);
        if panes > 1 {
            split_of(&mut app, &IDS[..panes]);
        }
        open(&mut app, "Fix the build");
        app.frames(30);
        let scope = if panes > 1 { "#split-pane-a1" } else { "#chat-session" };
        let list = format!("{scope} >> #transcript");
        app.scroll(&list, -2500.);
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
        }
        let view = app.bounds(&list);
        let node = app.target(&list).unwrap();
        let opts = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: false };
        let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, Some(node), &opts);
        let reading = zgx_script::driver::tree_lines_text(&lines, hidden)
            .lines()
            .filter_map(|l| {
                let id = l.split_whitespace().next()?.to_owned();
                let (_, rest) = l.split_once("text \"")?;
                let (words, at) = rest.split_once("\" ")?;
                let y: f32 = at.split_whitespace().next()?.split(',').nth(1)?.parse().ok()?;
                (y >= view.y && words.len() > 12).then_some((y, id))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(_, id)| id)
            .expect("text in view");
        let before = app.bounds(&reading);
        let seen = drag_sidebar(&mut app, 240., |app| {
            let b = app.bounds(&reading);
            (b.y, b.height)
        });
        assert!(seen.iter().any(|(_, h)| *h > before.height), "{panes} pane(s): the drag rewrapped what's read: {seen:?}");
        for (y, h) in &seen {
            assert!((y - before.y).abs() <= 2., "{panes} pane(s): {reading} stood at {} and went to {y}, {h} tall: {seen:?}", before.y);
        }
    }
}

// ── Live reflow: what a frame of a moving column costs ─────────────────────

/// Script calls a frame as the sidebar's edge is dragged, over 400 sessions
/// and `panes` long chats.
fn drag_script(panes_n: usize) -> u64 {
    drag_script_of(&ui().join("shell_fixture.luau"), panes_n)
}

fn drag_script_of(fixture: &Path, panes_n: usize) -> u64 {
    const IDS: [&str; 4] = ["a1", "a2", "a3", "b1"];
    let mut app = mount_at(fixture, &scratch(&format!("drag-script-{panes_n}")));
    // (Room for the panes beside the sidebar: short of it, it steps aside.)
    app.ui.scene.borrow_mut().resize(DRAG_ROOM, H);
    app.frames(10);
    assert!(app.host.run_command("many"));
    app.frames(30);
    long_chats(&app, &IDS, None);
    split_of(&mut app, &IDS[..panes_n]);
    open(&mut app, "Fix the build");
    app.frames(60);
    let calls = app.calls();
    drag_sidebar(&mut app, 120., |_| ());
    (app.calls() - calls) / 20
}

/// The sidebar's edge dragged, the sidebar's list isn't built again, and
/// each pane adds only its own few script calls. (A ref's field read in the
/// list's shape followed its whole box: about 110 more calls and 420 µs, at
/// every frame of a drag or of a window resized.)
#[test]
fn a_dragged_sidebar_edge_builds_no_list() {
    let (one, four) = (drag_script(1), drag_script(4));
    assert!(one <= 170, "{one} script calls a frame with one chat");
    assert!(four - one <= 3 * 45, "{one} script calls a frame with one chat, {four} with four");
}

/// v2 too: its sidebar's list isn't built again at each frame of the edge
/// dragged (which woke every row's bindings: 225 script calls a frame, 115 without).
#[test]
fn v2_s_dragged_sidebar_edge_builds_no_list() {
    let t = scratch("v2-drag").join("templates");
    live::sync_zeron(&ui(), &t).unwrap();
    let calls = drag_script_of(&compose(&t, "zeron-v2").join("shell_fixture.luau"), 1);
    assert!(calls <= 170, "{calls} script calls a frame");
}

/// A window resized by its edge reflows the page at every frame, as the
/// panel's glide and an edge dragged do: nothing holds it at one width.
#[test]
fn a_window_resized_reflows_the_page_live() {
    let mut app = mount(&scratch("resize-live"));
    long_chats(&app, &["a1"], None);
    open(&mut app, "Fix the build");
    app.frames(30);
    let mut widths = std::collections::BTreeSet::new();
    for i in 1..=20 {
        app.ui.scene.borrow_mut().resize(W - 8. * i as f32, H);
        app.frame();
        widths.insert(app.bounds("#chat-session >> #transcript").width.round() as i64);
    }
    assert_eq!(widths.len(), 20, "the transcript took {widths:?}");
}

/// A row in the sidebar follows the column's width: its edge dragged wider,
/// a row and the words in it are that much wider.
#[test]
fn a_sidebar_row_and_its_words_follow_the_dragged_edge() {
    let mut app = mount(&scratch("row-width"));
    let row = "#shell-sidebar >> role:button \"Fix the build\"";
    let words = "#shell-sidebar >> role:button \"Fix the build\" >> ~\"Fix the build\"";
    let (row_was, words_right) = (app.bounds(row), app.bounds(words).x);
    drag_sidebar(&mut app, 60., |_| ());
    app.frames(10);
    let (row_now, words_now) = (app.bounds(row), app.bounds(words));
    assert!((row_now.width - row_was.width - 60.).abs() <= 1., "the row {} wide, was {}", row_now.width, row_was.width);
    assert!((words_now.x - words_right).abs() <= 1., "its words start where they did");
    assert!(words_now.x + words_now.width <= row_now.x + row_now.width + 1., "its words {words_now:?} end inside the row {row_now:?}");
}

/// How many boxes stand above `id`.
fn depth_of(app: &mut TestApp, id: &str) -> usize {
    let node = app.target(id).unwrap();
    let scene = app.ui.scene.borrow();
    let (mut depth, mut at) = (0, scene.parent(node));
    while let Some(a) = at {
        depth += 1;
        at = scene.parent(a);
    }
    depth
}

/// v3's card stands one box deeper than v2's page, its panes' room. Every box
/// between it and the window multiplied what a frame of the panel's glide
/// measured: three more had a toggle's frame measure 1442 nodes to v2's 782.
#[test]
fn the_page_stands_about_as_deep_as_v2_s() {
    let t = scratch("depth").join("templates");
    live::sync_zeron(&ui(), &t).unwrap();
    let mut v2 = mount_at(&compose(&t, "zeron-v2").join("shell_fixture.luau"), &scratch("depth-v2"));
    let mut v3 = mount(&scratch("depth-v3"));
    let (two, three) = (depth_of(&mut v2, "#shell-page"), depth_of(&mut v3, "#shell-page"));
    assert!(three <= two + 1, "#shell-page under {three} boxes, v2's under {two}");
}

/// Frames' work as a column moves, and where it went: p50/p95 (ms), the
/// most, script calls a frame, and (with `ZGX_PROFILE=1`) each phase's
/// share of a frame (µs).
fn reflow_frames(app: &mut TestApp, mut step: impl FnMut(&mut TestApp, usize), n: usize) -> String {
    let _ = zgx::profile::take();
    let calls = app.calls();
    let mut each = Vec::new();
    for i in 0..n {
        step(app, i);
        each.push(timed(app));
    }
    let per = (app.calls() - calls) / n as u64;
    let most = each.iter().copied().fold(0., f64::max);
    let (p50, p95) = spread(each);
    let phases: Vec<String> = zgx::profile::take()
        .into_iter()
        .filter(|(_, _, t)| !t.is_zero())
        .map(|(p, _, t)| format!("{} {:.0}", p.name(), t.as_secs_f64() * 1e6 / n as f64))
        .collect();
    format!("p50 {p50:.2} p95 {p95:.2} max {most:.2} ms, {per} calls/frame [{}]", phases.join(" "))
}

/// The panel opened and closed again, a frame at a time.
fn reflow_toggle(app: &mut TestApp) -> String {
    app.wait_until(|app| app.find("~\"Loading \"").is_none());
    app.frames(40);
    reflow_frames(
        app,
        |app, i| {
            if i % 30 == 0 {
                app.click("#titlebar >> role:button \"Panel\"");
            }
        },
        60,
    )
}

/// The sidebar's edge dragged out and back, a move a frame.
fn reflow_drag(app: &mut TestApp) -> String {
    let edge = app.bounds("#shell-sidebar >> \"Resize sidebar\"");
    let (x, y) = (edge.x + edge.width / 2., edge.y + edge.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    let steps: Vec<i32> = (1..=30).chain((0..30).rev()).collect();
    let out = reflow_frames(app, |app, i| {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + 4. * steps[i] as f32, y }, m);
    }, steps.len());
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, m);
    app.ui.input.forget_clicks();
    out
}

/// Chats `ids` side by side (one: alone).
fn split_of(app: &mut TestApp, ids: &[&str]) {
    if ids.len() > 1 {
        use zgx_script::Value;
        let kids: Vec<Value> = ids.iter().map(|id| Value::record([("kind", Value::from("leaf")), ("id", Value::from(*id))])).collect();
        let tree = Value::record([("kind", Value::from("row")), ("children", Value::List(std::rc::Rc::from(kids)))]);
        app.host.set_state("screens/shell/split.luau::inset_split_trees", Value::List(std::rc::Rc::from(vec![tree]))).unwrap();
    }
}

/// Live reflow, v2 against v3: a panel toggle over a chat of visuals and
/// over a long chat, and the sidebar's edge dragged with 1–6 long chats.
/// (`COSTS_UI`: another ui/ to compose; `REFLOW_ONLY`: v2 or v3.)
#[test]
#[ignore = "a measurement: ZGX_PROFILE=1 cargo test --release --bin zeron-zgx live_reflow_measured -- --ignored --nocapture"]
fn live_reflow_measured() {
    const IDS: [&str; 6] = ["a1", "a2", "a3", "b1", "b2", "m1"];
    let root = scratch("reflow");
    let t = root.join("templates");
    let from = std::env::var("COSTS_UI").map(PathBuf::from).unwrap_or_else(|_| ui());
    let only = std::env::var("REFLOW_ONLY").ok();
    live::sync_zeron(&from, &t).unwrap();
    for (label, id) in [("v2", "zeron-v2"), ("v3", live::BUILTIN)] {
        if only.as_deref().is_some_and(|o| o != label) {
            continue;
        }
        let at = root.join(format!("live-{label}"));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::rename(compose(&t, id), &at).unwrap();
        let fixture = at.join("shell_fixture.luau");
        let mut say = |what: String| eprintln!("REFLOW {label} {what}");
        // A panel toggle over a chat of visuals, and over a long chat.
        let mut app = mount_visuals(&fixture, &scratch(&format!("rv-{label}")), 1500.);
        open(&mut app, "Fix the build");
        reply_of_visuals(&app, &[("a1", true)]);
        say(format!("toggle, visuals: {}", reflow_toggle(&mut app)));
        say(format!("drag, visuals: {}", reflow_drag(&mut app)));
        let mut app = mount_at(&fixture, &scratch(&format!("rl-{label}")));
        app.ui.scene.borrow_mut().resize(1600., 1000.);
        assert!(app.host.run_command("many"));
        app.frames(30);
        long_chats(&app, &IDS, None);
        open(&mut app, "Fix the build");
        app.frames(60);
        say(format!("toggle, long chat: {}", reflow_toggle(&mut app)));
        // The sidebar dragged, 1–6 long chats side by side (v2: one).
        for n in if label == "v2" { vec![1] } else { vec![1, 2, 4, 6] } {
            let mut app = mount_at(&fixture, &scratch(&format!("rd-{label}-{n}")));
            app.ui.scene.borrow_mut().resize(1600., 1000.);
            assert!(app.host.run_command("many"));
            app.frames(30);
            long_chats(&app, &IDS, None);
            split_of(&mut app, &IDS[..n]);
            open(&mut app, "Fix the build");
            app.frames(60);
            let shown = panes(&mut app).max(1);
            say(format!("drag, {shown} pane(s): {}", reflow_drag(&mut app)));
            say(format!("toggle, {shown} pane(s): {}", reflow_toggle(&mut app)));
        }
    }
}

/// What one frame of a moving column does, read from a probed zgx
/// (docs/zgx-layout-probe.patch): its layout passes and what each measured,
/// its script calls by source line, how often each node was measured, and
/// the chain from the window down to a transcript. `REFLOW_CASE`: `panes<n>`
/// (the sidebar's edge dragged over n long chats), `visuals` (over a chat of
/// visuals) or `toggle` (a frame of the panel's glide); `REFLOW_V2=1`: v2.
///
///   git -C <a copy of zgx at the pin> apply docs/zgx-layout-probe.patch
///   REFLOW_CASE=panes6 cargo test --release --features layout_probe \
///     --config 'patch."https://github.com/zeronsh/zgx".zgx.path="<copy>/crates/zgx"' \
///     --config 'patch."https://github.com/zeronsh/zgx".zgx-script.path="<copy>/crates/zgx-script"' \
///     --bin zeron-zgx live_reflow_traced -- --ignored --nocapture
#[cfg(feature = "layout_probe")]
#[test]
#[ignore = "a diagnosis: needs the probed zgx above"]
fn live_reflow_traced() {
    use std::collections::{BTreeMap, HashMap};
    use zgx::scene::{NodeId, Scene};
    const IDS: [&str; 6] = ["a1", "a2", "a3", "b1", "b2", "m1"];
    let case = std::env::var("REFLOW_CASE").unwrap_or_else(|_| "panes6".into());
    let fixture = if std::env::var_os("REFLOW_V2").is_some() {
        let t = scratch("trace-v2").join("templates");
        live::sync_zeron(&ui(), &t).unwrap();
        compose(&t, "zeron-v2").join("shell_fixture.luau")
    } else {
        ui().join("shell_fixture.luau")
    };
    let panes: usize = case.strip_prefix("panes").and_then(|n| n.parse().ok()).unwrap_or(1);
    let mut app = if case == "visuals" {
        let mut app = mount_visuals(&fixture, &scratch("trace-v"), 1500.);
        open(&mut app, "Fix the build");
        reply_of_visuals(&app, &[("a1", true)]);
        app.wait_until(|app| app.find("~\"Loading \"").is_none());
        app
    } else {
        let mut app = mount_at(&fixture, &scratch("trace-p"));
        app.ui.scene.borrow_mut().resize(1600., 1000.);
        assert!(app.host.run_command("many"));
        app.frames(30);
        long_chats(&app, &IDS, None);
        split_of(&mut app, &IDS[..panes]);
        open(&mut app, "Fix the build");
        app
    };
    app.frames(60);
    let toggling = case == "toggle";
    let edge = app.bounds("#shell-sidebar >> \"Resize sidebar\"");
    let (x, y) = (edge.x + edge.width / 2., edge.y + edge.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    if toggling {
        app.click("#titlebar >> role:button \"Panel\"");
        app.frames(6);
    } else {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
        app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
        for step in 1..=8 {
            app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + 4. * step as f32, y }, m);
            app.frame();
        }
    }
    // The frame looked at.
    zgx::trace::set_enabled(true);
    let _ = (zgx::trace::take_probe(), zgx::trace::take_probe_log());
    if !toggling {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + 40., y }, m);
    }
    app.frame();
    let (measures, log) = (zgx::trace::take_probe(), zgx::trace::take_probe_log());
    zgx::trace::set_enabled(false);
    // Script calls, by where each function is defined.
    let (calls, log): (Vec<_>, Vec<_>) = log.into_iter().partition(|l| l.0 == u32::MAX - 1);
    let mut by_line: BTreeMap<&str, (u32, u64)> = BTreeMap::new();
    for (_, us, at) in &calls {
        let e = by_line.entry(at).or_default();
        e.0 += 1;
        e.1 += us;
    }
    let mut by_line: Vec<_> = by_line.into_iter().collect();
    by_line.sort_by_key(|(_, (_, us))| std::cmp::Reverse(*us));
    eprintln!("CALLS {} script calls, {} us", calls.len(), calls.iter().map(|c| c.1).sum::<u64>());
    for (at, (n, us)) in by_line.iter().take(20) {
        eprintln!("CALL {n}x {us} us {at}");
    }
    // Who each node is: itself or its nearest ancestor the tree names.
    let all = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: true };
    let lines = zgx_script::driver::tree_lines_text(&zgx_script::driver::tree_lines(&app.ui, &app.handle, None, &all).0, 0);
    let names: HashMap<u32, String> = lines
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let n: u32 = l.strip_prefix('@')?.split_whitespace().next()?.parse().ok()?;
            Some((n, l.split_whitespace().take(2).collect::<Vec<_>>().join(" ")))
        })
        .collect();
    let list = app.target(if panes > 1 { "#split-pane-a1 >> #transcript" } else { "#chat-session >> #transcript" }).unwrap();
    let scene = app.ui.scene.borrow();
    fn walk(s: &Scene, n: NodeId, out: &mut HashMap<u32, NodeId>) {
        out.insert(n.number(), n);
        for c in s.children(n) {
            walk(s, *c, out);
        }
    }
    let mut nodes = HashMap::new();
    walk(&scene, scene.root(), &mut nodes);
    let owner = |n: u32| -> String {
        let mut at = nodes.get(&n).copied();
        while let Some(a) = at {
            if let Some(name) = names.get(&a.number()) {
                return name.clone();
            }
            at = scene.parent(a);
        }
        "?".into()
    };
    // Each layout pass: what started it, and where its measures went.
    let mut passes: BTreeMap<u64, Vec<&(u32, u64, String)>> = BTreeMap::new();
    for l in &log {
        passes.entry(l.1).or_default().push(l);
    }
    eprintln!("PASSES {}", passes.len());
    for (pass, ls) in &passes {
        let started = ls.iter().find(|l| l.0 == u32::MAX).map_or("", |l| l.2.as_str());
        let mut owners: BTreeMap<String, usize> = BTreeMap::new();
        for l in ls.iter().filter(|l| l.0 != u32::MAX) {
            *owners.entry(owner(l.0)).or_default() += 1;
        }
        let mut owners: Vec<_> = owners.into_iter().collect();
        owners.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        eprintln!("PASS {pass}: {} measures; {started}", ls.len() - 1);
        eprintln!("     {}", owners.iter().take(8).map(|(o, n)| format!("{n} {o}")).collect::<Vec<_>>().join(" | "));
    }
    let total: u32 = measures.values().map(|v| v.0).sum();
    eprintln!("MEASURES {} nodes, {total} measures, {:.0} us of their own", measures.len(), measures.values().map(|v| v.1).sum::<f32>());
    // The chain from the window down to a transcript.
    let mut chain = Vec::new();
    let mut at = Some(list);
    while let Some(a) = at {
        chain.push(a);
        at = scene.parent(a);
    }
    for a in chain.iter().rev() {
        let kind: String = format!("{:?}", scene.kind(*a)).chars().take(22).collect();
        let s = scene.style(*a);
        eprintln!("CHAIN {:>4} @{} {kind} grow={} abs={} {}", measures.get(&a.number()).map_or(0, |v| v.0), a.number(), s.flex_grow, s.absolute, names.get(&a.number()).map_or("", |n| n.as_str()));
    }
}

/// zgx alone: how many measures a frame takes as the window's width
/// changes, against the nodes there are, for a few shapes of the same 40
/// wrapped paragraphs. (A repro for zgx.)
#[test]
#[ignore = "a diagnosis: cargo test --release --bin zeron-zgx measures_per_width_change -- --ignored --nocapture"]
fn measures_per_width_change() {
    let dir = scratch("measures");
    let para = "A paragraph long enough to wrap over a few lines at any of these widths, so its text lays out as a real reply's does, with more after it.";
    let shapes: [(&str, &str); 14] = [
        ("row > column(grow, basis 0)", "ui.row { w_full = true, ui.column { grow = 1, basis = 0, {TEXTS} } }"),
        ("row > column(grow, basis 0, min_w 0)", "ui.row { w_full = true, ui.column { grow = 1, basis = 0, min_w = 0, {TEXTS} } }"),
        ("absolute left, w 100% > column", "ui.div { w_full = true, h_full = true, ui.column { absolute = true, left = 0, top = 0, w = \"100%\", {TEXTS} } }"),
        ("2 rows > column(grow)", "ui.row { w_full = true, ui.column { grow = 1, ui.row { ui.column { grow = 1, {TEXTS} } } } }"),
        ("3 rows > column(grow)", "ui.row { w_full = true, ui.column { grow = 1, ui.row { ui.column { grow = 1, ui.row { ui.column { grow = 1, {TEXTS} } } } } } }"),
        ("absolute in absolute", "ui.div { w_full = true, h_full = true, ui.div { absolute = true, left = 0, right = 0, top = 0, bottom = 0, ui.column { absolute = true, left = 0, right = 0, top = 0, {TEXTS} } } }"),
        ("row > column(w = 50%)", "ui.row { w_full = true, ui.column { w = \"50%\", {TEXTS} } }"),
        ("column of texts", "ui.column { w_full = true, {TEXTS} }"),
        ("column > column(p)", "ui.column { w_full = true, ui.column { p = 12, gap = 8, {TEXTS} } }"),
        ("row > column(grow)", "ui.row { w_full = true, ui.column { grow = 1, {TEXTS} } }"),
        ("row > column(grow, min_w 0)", "ui.row { w_full = true, ui.column { grow = 1, min_w = 0, {TEXTS} } }"),
        ("4 nested columns(p)", "ui.column { w_full = true, ui.column { p = 4, ui.column { p = 4, ui.column { p = 4, {TEXTS} } } } }"),
        ("each text in a row > column(grow)", "ui.column { w_full = true, {ROWTEXTS} }"),
        ("absolute left/right > column", "ui.div { w_full = true, h_full = true, ui.column { absolute = true, left = 0, right = 0, top = 0, {TEXTS} } }"),
    ];
    for (at, (label, shape)) in shapes.into_iter().enumerate() {
        let texts: Vec<String> = (0..40).map(|_| format!("ui.text {{ wrap = true, \"{para}\" }}")).collect();
        let rowtexts: Vec<String> = (0..40).map(|_| format!("ui.row {{ ui.column {{ grow = 1, ui.text {{ wrap = true, \"{para}\" }} }} }}")).collect();
        let body = shape.replace("{TEXTS}", &texts.join(", ")).replace("{ROWTEXTS}", &rowtexts.join(", "));
        let file = dir.join(format!("m{at}.luau"));
        std::fs::write(&file, format!("return {body}\n")).unwrap();
        let app = TestApp::new({
            let file = file.clone();
            move |runtime| ScriptHost::new(runtime, file)
        });
        app.ui.scene.borrow_mut().resize(900., 4000.);
        app.frames(5);
        let nodes = {
            let scene = app.ui.scene.borrow();
            fn walk(s: &zgx::scene::Scene, n: zgx::scene::NodeId) -> usize {
                1 + s.children(n).iter().map(|c| walk(s, *c)).sum::<usize>()
            }
            walk(&scene, scene.root())
        };
        zgx::trace::set_kept(64);
        zgx::trace::set_enabled(true);
        let t = std::time::Instant::now();
        for i in 0..20 {
            app.ui.scene.borrow_mut().resize(900. - 3. * i as f32, 4000.);
            app.frame();
        }
        let ms = t.elapsed().as_secs_f64() * 1000. / 20.;
        let frames = zgx::trace::frames(20);
        zgx::trace::set_enabled(false);
        let measured: u32 = frames.iter().map(|f| f.layout.measured).sum::<u32>() / frames.len().max(1) as u32;
        eprintln!("MEASURES {label}: {nodes} nodes, {measured} measures a frame ({:.1} per node), {ms:.2} ms a frame", measured as f64 / nodes as f64);
    }
}

/// zgx alone: layout passes and measures a frame as the window's width
/// changes, against how many text inputs stand in a row of panes. (A repro
/// for zgx: each input whose width changed lays the scene out itself.)
#[cfg(feature = "layout_probe")]
#[test]
#[ignore = "a diagnosis: needs the probed zgx (live_reflow_traced says how)"]
fn passes_per_text_input() {
    let dir = scratch("passes");
    for n in [0usize, 1, 2, 4, 8] {
        let pane = "ui.column { grow = 1, basis = 0, min_w = 0, ui.column { grow = 1, min_h = 0, ui.text { wrap = true, \"Some words that wrap in the pane above its input, as a real one's would do.\" } }, ui.textarea { w_full = true, value = ui.state(\"v\", \"\") } }";
        let panes: Vec<&str> = (0..n.max(1)).map(|_| if n == 0 { "ui.column { grow = 1, basis = 0, min_w = 0 }" } else { pane }).collect();
        let file = dir.join(format!("p{n}.luau"));
        std::fs::write(&file, format!("return ui.row {{ w_full = true, h_full = true, {} }}\n", panes.join(", "))).unwrap();
        let app = TestApp::new({
            let file = file.clone();
            move |runtime| ScriptHost::new(runtime, file)
        });
        app.ui.scene.borrow_mut().resize(1400., 900.);
        app.frames(5);
        zgx::trace::set_kept(64);
        zgx::trace::set_enabled(true);
        let _ = zgx::trace::take_probe_log();
        let t = std::time::Instant::now();
        for i in 0..20 {
            app.ui.scene.borrow_mut().resize(1400. - 6. * i as f32, 900.);
            app.frame();
        }
        let ms = t.elapsed().as_secs_f64() * 1000. / 20.;
        let log = zgx::trace::take_probe_log();
        zgx::trace::set_enabled(false);
        let passes: std::collections::BTreeSet<u64> = log.iter().map(|l| l.1).collect();
        let measures = log.iter().filter(|l| l.0 != u32::MAX).count();
        eprintln!("INPUTS {n}: {:.1} layout passes a frame, {} measures a frame, {ms:.3} ms a frame", passes.len() as f64 / 20., measures / 20);
    }
}
