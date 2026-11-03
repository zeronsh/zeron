//! Zeron v3's split chats (ui/screens/shell/split.luau, page.luau), and
//! Zeron v2, the single-page layout, as the window runs them: mounted
//! headless on the shell's fixture (ui/shell_fixture.luau).
//!
//! A chat dragged from the sidebar onto a chat's edge sits beside it; a
//! click focuses a pane; its × takes it out; the split is kept across a
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

fn panes(app: &mut TestApp) -> usize {
    app.tree().matches("\"Close pane\"").count()
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
    assert_eq!(panes(&mut app), 0, "a chat alone has no pane heads");
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
fn a_click_focuses_a_pane_and_its_close_takes_it_out() {
    let mut app = mount(&scratch("focus"));
    open(&mut app, "Fix the build");
    let alone = app.bounds("shell-page");
    split_right(&mut app, "Tidy the tests");
    let right = app.bounds("shell-page");
    // A click on the left pane's head: it's focused, the page's card moves
    // to it.
    let head = Rect { x: alone.x + 60., y: alone.y + 8., width: 10., height: 10. };
    let button = PointerButton::Primary;
    let (x, y) = (head.x, head.y);
    pointer(&mut app, InputEvent::PointerMove { x, y });
    pointer(&mut app, InputEvent::PointerDown { x, y, button });
    pointer(&mut app, InputEvent::PointerUp { x, y, button });
    app.ui.input.forget_clicks();
    app.frames(20);
    let left = app.bounds("shell-page");
    assert!(near(left.x, alone.x) && near(left.width, right.width), "{left:?}");
    // Its × (the focused one's, at its head's right end: 8 in, 22 wide, the
    // head 34 tall): back to one chat, its page the whole of it again.
    let (x, y) = (left.x + left.width - 8. - 11., left.y + 17.);
    pointer(&mut app, InputEvent::PointerMove { x, y });
    pointer(&mut app, InputEvent::PointerDown { x, y, button });
    pointer(&mut app, InputEvent::PointerUp { x, y, button });
    app.ui.input.forget_clicks();
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
/// transcript's measure), as one chat's does. So a resize's script grows
/// with the panes, by about what one chat's costs each, never faster.
#[test]
fn resizing_split_panes_costs_each_about_what_one_chat_does() {
    fn resize_calls(app: &mut TestApp) -> u64 {
        app.frames(30);
        let before = app.calls();
        for w in (0..12).map(|i| W - 20. * i as f32) {
            app.ui.scene.borrow_mut().resize(w, H);
            app.frame();
        }
        app.ui.scene.borrow_mut().resize(W, H);
        app.frames(5);
        app.calls() - before
    }
    let mut app = mount(&scratch("resize"));
    open(&mut app, "Fix the build");
    let mut calls = vec![resize_calls(&mut app)];
    for title in ["Tidy the tests", "Write the docs", "Ship the release"] {
        split_right(&mut app, title);
        calls.push(resize_calls(&mut app));
    }
    assert_eq!(panes(&mut app), 4);
    eprintln!("resize script calls, 1 to 4 chats: {calls:?}");
    let alone = calls[0] as f64;
    for (n, c) in calls.iter().enumerate().skip(1) {
        let per = *c as f64 / (n + 1) as f64;
        assert!(per <= alone * 1.5, "{} chats cost {c} script calls in a resize ({per:.0} each), one alone {alone}: {calls:?}", n + 1);
    }
    let steps: Vec<i64> = calls.windows(2).skip(1).map(|w| w[1] as i64 - w[0] as i64).collect();
    assert!(steps.iter().all(|s| (*s as f64) <= alone * 1.5), "each pane more costs about one chat: {calls:?}");
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

/// A pane's head carried over another pane: its edge moves the pane there,
/// its middle and its composer mention it there.
#[test]
fn a_pane_s_head_moves_it_at_an_edge_and_attaches_in_the_middle_or_the_composer() {
    let mut app = mount(&scratch("head-targets"));
    open(&mut app, "Fix the build");
    let alone = app.bounds("shell-page");
    split_right(&mut app, "Tidy the tests");
    // The left pane's head (Fix the build), the right one focused.
    let head = (alone.x + 80., alone.y + 17.);
    let right = app.bounds("shell-page");
    carry(&mut app, head, middle(right), |app| assert_eq!(showing(app), (false, true), "its middle shows Drop to mention"));
    assert_eq!(panes(&mut app), 2, "still the two");
    assert!(mentioned(&mut app, "Fix the build"), "its middle attaches:\n{}", app.tree());
    let composer = middle(app.bounds("#chat-composer"));
    carry(&mut app, head, composer, |app| assert_eq!(showing(app), (false, true)));
    assert_eq!(panes(&mut app), 2);
    // Its bottom band (above its composer, between the side bands): the
    // pane moves under it, the two stacked.
    let low = (right.x + right.width / 2., composer.1 - 120.);
    carry(&mut app, head, low, |app| assert_eq!(showing(app), (true, false), "the bottom band shows where it lands"));
    assert_eq!(panes(&mut app), 2);
    let now = app.bounds("shell-page");
    assert!(now.width > right.width * 1.5 && now.height < right.height * 0.6, "stacked: {now:?} from {right:?}");
}

// ── Zeron v2 ────────────────────────────────────────────────────────────────

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

    // v3, the default: the page a card set in by the window's gap.
    let live_dir = compose(&t, live::BUILTIN);
    let storage = root.join("state");
    let mut app = mount_at(&live_dir.join("shell_fixture.luau"), &storage);
    open(&mut app, "Fix the build");
    let v3 = app.bounds("shell-page");
    assert!(near(v3.y, 4.), "v3's page is a card under the gap: {v3:?}");

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
    assert!(near(app.bounds("shell-page").y, 4.));
}
