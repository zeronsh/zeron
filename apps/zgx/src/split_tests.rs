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

/// The whole interface, a node a line (`tree` stops at 200).
fn full_tree(app: &mut TestApp) -> String {
    app.ui.prepare_frame();
    let opts = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: false };
    let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, None, &opts);
    zgx_script::driver::tree_lines_text(&lines, hidden)
}

fn panes(app: &mut TestApp) -> usize {
    full_tree(app).matches("\"Close pane\"").count()
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
/// transcript's measure, its composer's). So a resize's script grows with
/// the panes, each one more costing about what one pane does, never
/// faster. (A chat alone is the chat page, its fixture's transcript empty:
/// told, not compared.)
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
/// over another's (its own), "Drop to mention" over both, and the panel's
/// tab-group landing over a side chat's composer in the panel.
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
    // The panel's tab-group landing over a side chat's composer in it.
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    app.frames(30);
    app.click("#panel-strip >> role:button \"New Tab\"");
    app.frames(20);
    app.click("#panel-open-side-chat");
    app.frames(40);
    let side = full_tree(&mut app)
        .lines()
        .find_map(|l| l.split_whitespace().find(|w| w.starts_with("#lane-composer-") && !w.contains("split-")).map(str::to_owned))
        .expect("the side chat's composer in the panel");
    let c = app.bounds(&side);
    // (Into the side chat's composer, at the panel body's left edge: the
    // files tab would start a group there.)
    let at = (c.x + 20., c.y + c.height / 2.);
    let tab = middle(app.bounds("#panel-strip >> role:tab \"Files\""));
    carry(&mut app, tab, at, |app| {
        in_front(app, "panel-drop-main", &side, at).unwrap();
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
        assert!(has(app, &format!("{pane} >> role:button \"{}\"", c.title)), "{when}: {id}'s head says {}", c.title);
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
    let b = app.bounds(&format!("#split-pane-{id}"));
    let (x, y) = (b.x + b.width / 2., b.y + 17.);
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

/// 200 steps, from a fixed seed: focus a pane, open a chat from the
/// sidebar, split one in by a drop, close a pane, type a draft in the
/// focused composer, stream a reply (and a tool call) into a chat that
/// isn't focused, change a chat's status, a chat's agent asking, reload,
/// resize; and in the focused chat's panel: open a tab from any source,
/// focus a tab group, split one off, merge one into another. After each,
/// every pane shows exactly its own chat, live as if focused (its title,
/// transcript, reply, tools, status, question and draft), and one pane is
/// focused: the route's; and each tab opened landed in the panel's focused
/// group.
#[test]
fn two_hundred_random_steps_keep_every_pane_its_own_chat() {
    let mut app = mount(&scratch("random"));
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
                let groups = panel_groups_shown(&mut app);
                // (Focusing a group as often as there are groups to focus.)
                let pick = if groups.len() >= 2 { [0, 2, 2, 3, 4][next(5)] } else { [0, 1, 3][next(3)] };
                match pick {
                    0 | 1 => {
                        let source = ["pr", "file", "url", "terminal"][next(4)];
                        opens_into_focus(&mut app, &r, source, &format!("step {step}"));
                        format!("panel {r}: open a {source}")
                    }
                    2 if groups.len() >= 2 => {
                        let gid = groups[next(groups.len())].clone();
                        let tab = full_tree(&mut app)
                            .lines()
                            .skip_while(|l| !l.contains(&format!("#panel-group-{gid} ")))
                            .find_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("#panel-tab-")).map(str::to_owned));
                        let Some(tab) = tab else { continue };
                        app.click(&format!("#panel-tab-{tab}"));
                        app.frames(10);
                        assert_eq!(panel_focus(&app, &r).1, gid, "step {step}: a press on {tab} focuses its group");
                        format!("panel {r}: focus {gid}")
                    }
                    3 if groups.len() < 3 => {
                        let (active, focus) = panel_focus(&app, &r);
                        let Some(active) = active else { continue };
                        let over = if groups.len() >= 2 { format!("#panel-group-{focus}") } else { "#shell-files".to_string() };
                        let side = ["right", "bottom"][next(2)];
                        tab_to(&mut app, &active, &over, side);
                        format!("panel {r}: split {active} {side}")
                    }
                    _ if groups.len() >= 2 => {
                        let (active, focus) = panel_focus(&app, &r);
                        let Some(active) = active else { continue };
                        let into = groups.iter().find(|g| **g != focus).unwrap().clone();
                        tab_to(&mut app, &active, &format!("#panel-group-{into}"), "middle");
                        format!("panel {r}: merge {active} into {into}")
                    }
                    _ => continue,
                }
            }
            9 => {
                let w = 1100. + next(400) as f32;
                app.ui.scene.borrow_mut().resize(w, H);
                app.frames(6);
                app.ui.scene.borrow_mut().resize(W, H);
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
    for kind in ["open a", "focus g", ": focus", "focus main", "split tab", "merge tab", "split a", "focus a", "close", "reload", "asking true", "stream into"] {
        let n = log.iter().filter(|w| w.contains(kind)).count();
        eprintln!("{kind}: {n}");
    }
    let panel = log.iter().filter(|w| w.starts_with("panel ")).count();
    let group_focus = log.iter().filter(|w| w.starts_with("panel ") && w.contains(": focus")).count();
    assert!(panel >= 20 && group_focus >= 5, "the panel's groups got their share: {panel} of {}, {group_focus} focused", log.len());
}





// ── The panel's tab groups: one focus, every tab opened into it ────────────

/// A named state's value (`ui.state`s are named `module.luau::name`).
fn state_of(app: &TestApp, name: &str) -> zgx_script::Value {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n == name || n.ends_with(&format!("::{name}")))
        .map(|(_, v)| v)
        .unwrap_or_default()
}

fn text(v: Option<&zgx_script::Value>) -> Option<String> {
    match v? {
        zgx_script::Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Chat `chat`'s panel: its active tab, the focused group, and the group a
/// tab is in (by the groups as the panel keeps them; none: one group).
fn panel_focus(app: &TestApp, chat: &str) -> (Option<String>, String) {
    let key = format!("chat:{chat}");
    let sessions = state_of(app, "panel_sessions");
    let active = text(sessions.field(&key).and_then(|s| s.field("active")));
    let groups = state_of(app, "panel_groups");
    let focus = text(groups.field(&key).and_then(|r| r.field("focus"))).unwrap_or_else(|| "main".into());
    (active, focus)
}

fn group_of_tab(app: &TestApp, chat: &str, tab: &str) -> String {
    let groups = state_of(app, "panel_groups");
    let Some(zgx_script::Value::Map(gs)) = groups.field(&format!("chat:{chat}")).and_then(|r| r.field("groups")).cloned() else {
        return "main".into();
    };
    for (gid, g) in gs.iter() {
        if let Some(zgx_script::Value::List(tabs)) = g.field("tabs")
            && tabs.iter().any(|t| matches!(t, zgx_script::Value::Str(s) if s.as_ref() == tab))
        {
            return gid.to_string();
        }
    }
    "main".into()
}

/// The groups showing, by their ids (`#panel-group-<gid>`).
fn panel_groups_shown(app: &mut TestApp) -> Vec<String> {
    let mut out: Vec<String> = full_tree(app).lines().filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("#panel-group-")).map(str::to_owned)).collect();
    out.sort();
    out.dedup();
    out
}

/// A panel tab (its id) carried to `side` of the panel body or of group
/// `gid`'s body: a new group there.
fn tab_to(app: &mut TestApp, tab: &str, over: &str, side: &str) {
    let b = app.bounds(over);
    // (The body: under the strip's band.)
    let (top, h) = (b.y + 40., b.height - 40.);
    let to = match side {
        "right" => (b.x + b.width - 12., top + h / 2.),
        "left" => (b.x + 12., top + h / 2.),
        "bottom" => (b.x + b.width / 2., top + h - 12.),
        _ => (b.x + b.width / 2., top + h / 2.),
    };
    let from = middle(app.bounds(&format!("#panel-tab-{tab}")));
    carry(app, from, to, |_| {});
}

/// Opening a tab from source `source` lands it in the focused group: the
/// new tab active, in the group that was focused, still focused.
fn opens_into_focus(app: &mut TestApp, chat: &str, source: &str, when: &str) {
    let (_, focus) = panel_focus(app, chat);
    assert!(app.host.run_command(&format!("fixture.open_{source}")), "{when}: fixture.open_{source}");
    app.frames(20);
    let (active, now) = panel_focus(app, chat);
    let active = active.unwrap_or_else(|| panic!("{when}: {source} opened no tab"));
    assert_eq!(now, focus, "{when}: opening a {source} moved the focus");
    assert_eq!(group_of_tab(app, chat, &active), focus, "{when}: the {source} landed outside the focused group");
    if panel_groups_shown(app).len() >= 2 {
        assert!(has(app, &format!("#panel-group-{focus} >> #panel-tab-{active}")), "{when}: the {source}'s tab shows in the focused group");
    }
}

#[test]
fn every_tab_opened_lands_in_the_focused_panel_group() {
    let mut app = mount(&scratch("groups"));
    open(&mut app, "Fix the build");
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    app.frames(30);
    assert!(app.host.run_command("fixture.open_terminal"));
    app.frames(20);
    let (term, _) = panel_focus(&app, "a1");
    let term = term.unwrap();
    // Two groups: the terminal to the panel's right.
    tab_to(&mut app, &term, "#shell-files", "right");
    assert_eq!(panel_groups_shown(&mut app).len(), 2, "two groups");
    for round in 0..2 {
        for gid in panel_groups_shown(&mut app) {
            // Focused by a press on a tab of its own.
            let (_, before) = panel_focus(&app, "a1");
            let tab = full_tree(&mut app)
                .lines()
                .skip_while(|l| !l.contains(&format!("#panel-group-{gid} ")))
                .find_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("#panel-tab-")).map(str::to_owned))
                .expect("a tab in the group");
            app.click(&format!("#panel-tab-{tab}"));
            app.frames(10);
            assert_eq!(panel_focus(&app, "a1").1, gid, "a press focuses its group (from {before})");
            for source in ["pr", "file", "url", "terminal"] {
                opens_into_focus(&mut app, "a1", source, &format!("round {round}, {gid}"));
            }
        }
        if round == 0 {
            // Three: one of the right group's tabs under it.
            let right = panel_groups_shown(&mut app).into_iter().find(|g| g != "main").unwrap();
            let (active, _) = panel_focus(&app, "a1");
            tab_to(&mut app, &active.unwrap(), &format!("#panel-group-{right}"), "bottom");
            assert_eq!(panel_groups_shown(&mut app).len(), 3, "three groups");
        }
    }
    // The focus survives a reload and a switch of chats.
    let (_, focus) = panel_focus(&app, "a1");
    app.host.reload().expect("the reload failed");
    app.frames(30);
    assert_eq!(panel_focus(&app, "a1").1, focus, "a reload keeps the focus");
    opens_into_focus(&mut app, "a1", "pr", "after a reload");
    open(&mut app, "Tidy the tests");
    open(&mut app, "Fix the build");
    app.frames(30);
    assert_eq!(panel_focus(&app, "a1").1, focus, "a switch of chats keeps it");
    opens_into_focus(&mut app, "a1", "pr", "after a switch");
    // A tab already open (the same PR) focuses where it is.
    let groups = panel_groups_shown(&mut app);
    let other = groups.iter().find(|g| **g != focus).unwrap().clone();
    let tab = full_tree(&mut app)
        .lines()
        .skip_while(|l| !l.contains(&format!("#panel-group-{other} ")))
        .find_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("#panel-tab-")).map(str::to_owned))
        .unwrap();
    app.click(&format!("#panel-tab-{tab}"));
    app.frames(10);
    assert_eq!(panel_focus(&app, "a1").1, other);
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



/// A panel toggle over a chat of visuals lays the chat out once a way, not
/// at every frame of its glide: the transcript and a chart in it take at
/// most three widths through an open and a close (as they were, beside the
/// panel, and back), and the toggle runs about the script a plain chat's
/// does. (Shipping's way: what's beside the moving column keeps its width;
/// the card, a box, follows the edge.)
#[test]
fn a_panel_toggle_lays_a_chat_of_visuals_out_once_a_way() {
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
                    let plot = app.target(".chart-plot").or_else(|_| app.target("#chat-session >> .chart-plot"));
                    if let Ok(node) = plot {
                        chart.insert(app.ui.scene.borrow().bounds(node).width.round() as i64);
                    }
                }
            }
        }
        (app.calls() - calls, transcript, chart)
    };
    let (plain, plain_widths, _) = toggle(false);
    let (with, widths, chart) = toggle(true);
    assert!(plain_widths.len() <= 3 && widths.len() <= 3, "the transcript took {widths:?} (plain {plain_widths:?})");
    assert!(chart.len() <= 3, "a chart took {chart:?}");
    assert!(with as f64 <= plain as f64 * 1.25, "{with} script calls over visuals, {plain} over a plain chat");
}


// ── A new chat's page going flat ────────────────────────────────────────────

/// The sidebar closing over a new chat's page: the card's fill and edge
/// fade out with the sidebar's own motion, and back as it opens, never in
/// one frame; the gap round the card eases with them.
#[test]
fn a_new_chat_s_card_fades_with_the_sidebar_never_in_one_frame() {
    let mut app = mount(&scratch("flat"));
    app.frames(30);
    let sample = |app: &mut TestApp| -> (Vec<f32>, Vec<f32>) {
        let (mut fill, mut gap) = (Vec::new(), Vec::new());
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(8));
            app.frame();
            fill.push(app.opacity("shell-page-card"));
            gap.push(app.bounds("shell-page").y);
        }
        (fill, gap)
    };
    let between = |v: &[f32], lo: f32, hi: f32| v.iter().filter(|x| **x > lo + 0.01 && **x < hi - 0.01).count();
    assert!((app.opacity("shell-page-card") - 1.).abs() < 0.01, "a card with the sidebar open");
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    let (fill, gap) = sample(&mut app);
    assert!(between(&fill, 0., 1.) >= 2, "it fades out over frames: {fill:?}");
    assert!(between(&gap, 0., 4.) >= 2, "the gap eases: {gap:?}");
    assert!(fill.last().unwrap().abs() < 0.01 && gap.last().unwrap().abs() < 0.5, "flat at last");
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    let (fill, _) = sample(&mut app);
    assert!(between(&fill, 0., 1.) >= 2, "and back in: {fill:?}");
    assert!((fill.last().unwrap() - 1.).abs() < 0.01);
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

/// Dragging the sidebar's edge, what's in the page keeps one width: no
/// transcript, composer or pane is laid out again at each move (each holds
/// the width it had as the drag began, its card following the edge), and
/// a frame's script (the sidebar's own, following its width) is the same
/// whatever the panes; let go, each takes its own width once, the reading
/// place where it stood.
#[test]
fn a_dragged_edge_lays_the_page_out_once_on_release_keeping_the_reading_place() {
    let mut script = Vec::new();
    for panes in [1usize, 4] {
        const IDS: [&str; 4] = ["a1", "a2", "a3", "b1"];
        let mut app = mount(&scratch(&format!("edge-{panes}")));
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
        let calls = app.calls();
        let widths = drag_sidebar(&mut app, 120., |app| app.bounds(&list).width.round() as i64);
        let per_frame = (app.calls() - calls) / 20;
        let distinct: std::collections::BTreeSet<i64> = widths.iter().copied().collect();
        assert_eq!(distinct.len(), 1, "{panes} pane(s): the transcript held one width through the drag: {widths:?}");
        script.push(per_frame);
        // Let go: its own width, once.
        let mut after = Vec::new();
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
            after.push(app.bounds(&list).width.round() as i64);
        }
        let settled: std::collections::BTreeSet<i64> = after.iter().copied().collect();
        assert_eq!(settled.len(), 1, "{panes} pane(s): laid out once on release: {after:?}");
        assert_ne!(after[0], widths[0], "{panes} pane(s): and to its own width");
        let now = app.bounds(&reading);
        assert!((now.y - before.y).abs() <= 2., "{panes} pane(s): the reading place kept: {reading} from {} to {}", before.y, now.y);
    }
    assert!(script[1] as f64 <= script[0] as f64 * 1.1 + 4., "a frame of the drag's script, 1 pane then 4: {script:?}");
}
