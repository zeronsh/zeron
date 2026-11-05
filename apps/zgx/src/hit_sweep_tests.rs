//! Every control the main window shows takes the pointer where it's drawn
//! (ui/screens/shell, on its fixture ui/shell_fixture.luau, with a long
//! transcript in a session: ui/hit_fixture.luau). Tests click a
//! control by its target, one at a time, so a layer laid over the page that
//! takes the pointer (invisible, or under-ink) went unnoticed by them: this
//! sweep presses nothing, but moves the pointer to the visible middle of
//! every interactive node (buttons, tabs, rows, fields, scroll views,
//! splitters) and asks the window's own routing (`Ui::dispatch`, as the
//! desktop host delivers a real mouse) where that pointer goes. It must reach
//! the control, something inside it, or the control's own container that
//! listens for it; anything else covers it, and is listed with what does.
//!
//! In the window's states that matter: a new chat, a session (the panel
//! closed and open), Settings; full size and narrow. And a wheel over the
//! transcript scrolls it.

use zgx::{input::InputEvent, scene::{NodeId, Rect}, semantics::Role};
use zgx_script::{ScriptHost, testing::TestApp};

fn mount(w: f32, h: f32) -> TestApp {
    mount_fixture("ui/hit_fixture.luau", w, h)
}

fn mount_fixture(fixture: &str, w: f32, h: f32) -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(fixture);
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(w, h);
    app.frames(40);
    app
}

/// The roles a person points at.
fn interactive(role: &Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Link
            | Role::CheckBox
            | Role::Slider
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::ScrollView
            | Role::Splitter
            | Role::MenuItem
            | Role::ListBoxOption
            | Role::Tab
            | Role::RadioButton
            | Role::Switch
            | Role::SpinButton
            | Role::ComboBox
            | Role::TreeItem
    )
}

/// The edges a drag resizes columns at (the sidebar's, the panel's).
const EDGES: &[&str] = &["\"Resize sidebar\"", "\"Resize files\""];

/// A control the pointer doesn't reach, and what it reaches instead.
#[derive(Debug)]
struct Covered {
    control: String,
    at: (f32, f32),
    /// Where the pointer goes there (the routing's target), and the topmost
    /// thing drawn there.
    reaches: String,
    top: String,
}

/// How the test tree names node `id` ("@12 #id role \"label\""), else its
/// number.
fn name(tree: &str, id: Option<NodeId>) -> String {
    let Some(id) = id else { return "nothing".into() };
    let prefix = format!("@{} ", id.number());
    tree.lines()
        .map(str::trim)
        .find(|line| line.starts_with(&prefix))
        .map(|line| {
            // (Its box at the end says nothing here.)
            let words: Vec<&str> = line.split(' ').collect();
            let cut = words.iter().rposition(|w| w.contains(',') && w.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-'));
            words[..cut.unwrap_or(words.len())].join(" ")
        })
        .unwrap_or_else(|| format!("@{}", id.number()))
}

/// Whether node `id` and its ancestors draw it at all (none fully clear).
fn drawn(app: &TestApp, id: NodeId) -> bool {
    let scene = app.ui.scene.borrow();
    let mut node = Some(id);
    while let Some(n) = node {
        if scene.effects(n).opacity <= 0.01 {
            return false;
        }
        node = scene.parent(n);
    }
    true
}

/// Every interactive control showing, by where the pointer goes at its
/// visible middle; those it doesn't reach.
fn sweep(app: &mut TestApp, w: f32, h: f32) -> (usize, Vec<Covered>) {
    app.frames(4);
    let window = Rect { x: 0., y: 0., width: w, height: h };
    let mut controls: Vec<(NodeId, String)> = {
        let semantics = app.ui.semantics.borrow();
        semantics
            .iter()
            // (One that can't be used now takes no pointer, by design.)
            .filter(|(_, node)| interactive(&node.role) && !node.disabled)
            .map(|(id, node)| (id, format!("{:?} \"{}\"", node.role, node.label)))
            .collect()
    };
    // (In the scene's order, so every run sweeps alike.)
    controls.sort_by_key(|(id, _)| id.number());
    // The edges that resize a column: named, not a role of their own.
    for edge in EDGES {
        if let Some(id) = app.find(edge) {
            controls.push((id, format!("edge {edge}")));
        }
    }
    let tree = app.tree();
    let mut checked = 0;
    let mut gone = 0;
    let mut covered = Vec::new();
    for (id, label) in controls {
        // (Gone since the list was taken: the page rebuilt it.)
        if !app.ui.scene.borrow().contains(id) {
            gone += 1;
            continue;
        }
        let visible = app.ui.scene.borrow().visible_bounds(id);
        let Some(b) = visible.and_then(|b| intersect(b, window)) else { continue };
        if b.width < 2. || b.height < 2. || !drawn(app, id) {
            continue;
        }
        let at = (b.x + b.width / 2., b.y + b.height / 2.);
        let target = app.ui.dispatch(InputEvent::PointerMove { x: at.0, y: at.1 }).target;
        let top = app.ui.scene.borrow().hit_test(at.0, at.1);
        // (Away again at once: a hover opens nothing for the next one.)
        app.ui.dispatch(InputEvent::PointerMove { x: -10., y: -10. });
        checked += 1;
        if std::env::var("HIT_SWEEP_LIST").is_ok() {
            println!("    swept {label} at ({:.0}, {:.0})", at.0, at.1);
        }
        let reached = target.is_some_and(|t| {
            let scene = app.ui.scene.borrow();
            scene.contains(id) && scene.contains(t) && (scene.is_within(t, id) || scene.is_within(id, t))
        });
        if !reached {
            covered.push(Covered { control: label, at, reaches: name(&tree, target), top: name(&tree, top) });
        }
    }
    // (The pointer gone, what a hover began settles before the next state.)
    app.ui.dispatch(InputEvent::PointerMove { x: -10., y: -10. });
    app.frames(30);
    assert!(gone * 4 <= checked.max(1), "{gone} controls went away mid-sweep ({checked} swept): the page rebuilt under the pointer");
    (checked, covered)
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let r = (a.x + a.width).min(b.x + b.width);
    let btm = (a.y + a.height).min(b.y + b.height);
    (r > x && btm > y).then(|| Rect { x, y, width: r - x, height: btm - y })
}

/// The window's states, in order, at `w` × `h`: each swept; what's covered,
/// by state.
fn states(w: f32, h: f32) -> Vec<(String, usize, Vec<Covered>)> {
    let mut app = mount(w, h);
    let mut out = Vec::new();
    let mut take = |app: &mut TestApp, state: &str| {
        let (n, covered) = sweep(app, w, h);
        out.push((format!("{state} at {w}×{h}"), n, covered));
    };
    take(&mut app, "a new chat");
    if app.find("#shell-sidebar >> role:button \"Fix the build\"").is_some() {
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
    } else {
        // (Narrow: the sidebar is put away; the session by its route.)
        assert!(app.host.run_command("shell.session_1"), "no shell.session_1 command");
    }
    assert!(app.wait_until(|app| app.find("#transcript").is_some()), "the session didn't open at {w}×{h}");
    app.frames(20);
    take(&mut app, "a session, its panel closed");
    assert!(app.find("#titlebar >> role:button \"Panel\"").is_some(), "no Panel toggle in the session at {w}×{h}");
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    take(&mut app, "a session, its panel open");
    // (A shortcut fires through the host's menu bar, which a headless app
    // has none of: the command itself.)
    assert!(app.host.run_command("shell.settings"), "no shell.settings command");
    app.frames(40);
    take(&mut app, "Settings");
    out
}

fn report(all: &[(String, usize, Vec<Covered>)]) -> String {
    let mut text = String::new();
    for (state, n, covered) in all {
        text += &format!("{state}: {n} controls, {} covered\n", covered.len());
        for c in covered {
            text += &format!(
                "  {} at ({:.0}, {:.0}): the pointer reaches {}; on top: {}\n",
                c.control, c.at.0, c.at.1, c.reaches, c.top
            );
        }
    }
    text
}

#[test]
fn every_control_takes_the_pointer_where_it_is_drawn() {
    let mut all = states(1240., 820.);
    all.extend(states(720., 820.));
    let text = report(&all);
    println!("{text}");
    assert!(all.iter().all(|(_, n, _)| *n > 0), "a state showed no controls:\n{text}");
    assert!(all.iter().all(|(_, _, covered)| covered.is_empty()), "controls the pointer doesn't reach:\n{text}");
}

#[test]
fn a_wheel_over_the_transcript_scrolls_it() {
    let mut app = mount(1240., 820.);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    assert!(app.wait_until(|app| app.find("#transcript").is_some()), "the session's transcript never showed");
    app.frames(20);
    app.frames(20);
    let view = app.bounds("#transcript");
    let at = (view.x + view.width / 2., view.y + view.height / 2.);
    // Where the transcript's content is, by its first child's top.
    let transcript = scene_id(&mut app, "#transcript");
    // Where the transcript's content is: the tops of everything in it.
    let first = |app: &mut TestApp| -> Vec<i32> {
        let scene = app.ui.scene.borrow();
        let mut tops = Vec::new();
        let mut stack = vec![transcript];
        while let Some(n) = stack.pop() {
            tops.push(scene.bounds(n).y.round() as i32);
            stack.extend(scene.children(n).iter().copied());
        }
        tops
    };
    // (Settled first: a session opens on its end, eased there.)
    app.frames(120);
    let before = first(&mut app);
    // Up first (a session opens at its end), then down; each looked at
    // as it lands and once settled.
    app.ui.dispatch(InputEvent::Scroll { x: at.0, y: at.1, delta_x: 0., delta_y: -300. });
    app.frames(2);
    let up_now = first(&mut app);
    app.frames(30);
    let up = first(&mut app);
    app.ui.dispatch(InputEvent::Scroll { x: at.0, y: at.1, delta_x: 0., delta_y: 300. });
    app.frames(30);
    let down = first(&mut app);
    println!("wheel: moved at once {}, still moved {}, back down {}", up_now != before, up != before, down != up);
    assert!(
        up != before || down != up,
        "a wheel at the transcript's middle {at:?} moved nothing ({} nodes in it)",
        before.len()
    );
}

fn scene_id(app: &mut TestApp, target: &str) -> NodeId {
    app.target(target).unwrap_or_else(|e| panic!("{target}: {e}"))
}

/// The sweep catches what it's for: an invisible layer laid over the page
/// that takes the pointer is named over the controls it covers; one that
/// lets the pointer through covers nothing.
#[test]
fn the_sweep_names_a_layer_that_takes_the_pointer() {
    let (w, h) = (1240., 820.);
    let mut app = mount_fixture("ui/hit_cover_fixture.luau", w, h);
    assert!(app.host.run_command("hit.cover"));
    app.frames(10);
    let (n, covered) = sweep(&mut app, w, h);
    assert!(n > 10, "the controls weren't found ({n})");
    assert!(
        covered.len() >= n - 1 && covered.iter().all(|c| c.reaches.contains("#hit-cover")),
        "the cover wasn't named over every control:\n{}",
        report(&[("covered".into(), n, covered)])
    );
    assert!(app.host.run_command("hit.cover_through"));
    app.frames(10);
    let (n, covered) = sweep(&mut app, w, h);
    assert!(covered.is_empty(), "a layer the pointer passes through covered:\n{}", report(&[("through".into(), n, covered)]));
}
