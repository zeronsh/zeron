//! A session dropped in the sidebar lands in one motion: profiled frame by
//! frame from the release (ui/shell_fixture.luau, its host answering a
//! place at once, or a beat later).

use zgx::input::{InputEvent, Modifiers, PointerButton};
use zgx::scene::NodeId;
use zgx_script::{ScriptHost, testing::TestApp};

pub(crate) fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    app
}

pub(crate) const TITLES: &[&str] = &["Fix the build", "Write the docs", "Tidy the tests", "Ship the release", "Answer the review"];

/// A row as one frame shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seen {
    pub node: Option<NodeId>,
    pub y: f32,
    pub opacity: f32,
    pub copies: usize,
}

fn opacity(app: &TestApp, mut id: NodeId) -> f32 {
    let scene = app.ui.scene.borrow();
    let mut o = 1.;
    loop {
        o *= scene.effects(id).opacity;
        match scene.parent(id) {
            Some(p) => id = p,
            None => return o,
        }
    }
}

pub(crate) fn seen(app: &mut TestApp, title: &str) -> Seen {
    let target = zgx_script::driver::parse(&format!("#shell-sidebar >> role:button \"{title}\"")).unwrap();
    app.ui.prepare_frame();
    let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
    let visible: Vec<NodeId> = found
        .iter()
        .copied()
        .filter(|&id| app.ui.scene.borrow().layout_visible(id))
        .collect();
    match visible.first() {
        Some(&id) => {
            let b = app.ui.scene.borrow().transformed_bounds(id);
            Seen { node: Some(id), y: b.y, opacity: opacity(app, id), copies: visible.len() }
        }
        None => Seen { node: None, y: f32::NAN, opacity: 0., copies: 0 },
    }
}

/// A drop, frame by frame: every row as it stood just before the release
/// (`rows[0]`) and on each frame after; where the preview stood, and where
/// the dropped row stood before the drag began.
pub(crate) struct Drop {
    pub title: String,
    pub preview_y: f32,
    pub started_y: f32,
    pub rows: Vec<Vec<(String, Seen)>>,
}

/// Drags `title`'s row to the point `to` returns (asked once the drag is
/// under way, the list as the drag has it) and lets go there. `during`
/// runs after each frame that follows (its index from 1): the engine's
/// answers arriving mid-motion.
pub(crate) fn drop_at(
    app: &mut TestApp,
    title: &str,
    to: impl Fn(&mut TestApp) -> (f32, f32),
    after: usize,
    mut during: impl FnMut(&mut TestApp, usize),
) -> Drop {
    let row = app.target(&format!("#shell-sidebar >> role:button \"{title}\"")).expect("the row");
    let b = app.ui.scene.borrow().hit_bounds(row);
    let started_y = b.y;
    let (x, y) = (b.x + 40., b.y + b.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    for step in 1..=3 {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + 4. * step as f32, y: y + 4. * step as f32 }, m);
        app.frames(2);
    }
    // (The sidebar takes up the drag a beat after it starts: `ui.after`,
    // on the clock.)
    std::thread::sleep(std::time::Duration::from_millis(30));
    app.frames(20);
    let (tx, ty) = to(app);
    let (from_x, from_y) = (x + 12., y + 12.);
    for step in 1..=8 {
        let t = step as f32 / 8.;
        app.ui.dispatch_with_modifiers(
            InputEvent::PointerMove { x: from_x + (tx - from_x) * t, y: from_y + (ty - from_y) * t },
            m,
        );
        app.frames(3);
    }
    app.frames(30);
    let preview_y = {
        let drag = app.ui.input.drag_state().expect("a drag under way");
        let preview = drag.preview.expect("its preview");
        drag.preview_origin.map_or_else(|| app.ui.scene.borrow().transformed_bounds(preview).y, |(_, y)| y)
    };
    let all = |app: &mut TestApp| TITLES.iter().map(|t| (t.to_string(), seen(app, t))).collect::<Vec<_>>();
    let mut rows = vec![all(app)];
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: tx, y: ty, button }, m);
    app.ui.input.forget_clicks();
    app.settle();
    for i in 1..=after {
        app.frame();
        rows.push(all(app));
        during(app, i);
    }
    Drop { title: title.into(), preview_y, started_y, rows }
}

/// What, if anything, breaks the one motion: the dropped row a new node,
/// doubled, hidden or back at its old place, or anything going the wrong
/// way on its road to rest.
pub(crate) fn faults(what: &str, d: &Drop, lands_home: bool) -> Vec<String> {
    let mut out = Vec::new();
    let names: Vec<String> = d.rows[0].iter().map(|(t, _)| t.clone()).collect();
    for (i, name) in names.iter().enumerate() {
        let track: Vec<&Seen> = d.rows.iter().map(|r| &r[i].1).collect();
        let dropped = *name == d.title;
        // Its road: from where it stood (the dropped row: its preview).
        let mut ys: Vec<f32> = Vec::new();
        ys.push(if dropped { d.preview_y } else { track[0].y });
        for (f, s) in track.iter().enumerate().skip(1) {
            if s.copies != 1 {
                out.push(format!("{what}: frame {f}: \"{name}\" shows {} times", s.copies));
            }
            if dropped && s.opacity < 0.99 {
                out.push(format!("{what}: frame {f}: \"{name}\" isn't there yet (ink {:.2})", s.opacity));
            }
            if s.node != track[0].node {
                out.push(format!("{what}: frame {f}: \"{name}\" was made again ({:?} → {:?})", track[0].node, s.node));
            }
            ys.push(s.y);
        }
        let rest = *ys.last().unwrap();
        let way = (rest - ys[0]).signum();
        // (A motion, not a jump: no frame covers more than half the road.)
        let road = (rest - ys[0]).abs();
        if road > 4. {
            for (f, pair) in ys.windows(2).enumerate() {
                if (pair[1] - pair[0]).abs() > road / 2. {
                    out.push(format!("{what}: frame {}: \"{name}\" jumped {} → {} (on its way {} → {rest})", f + 1, pair[0], pair[1], ys[0]));
                }
            }
        }
        for (f, pair) in ys.windows(2).enumerate() {
            if way != 0. && (pair[1] - pair[0]) * way < -0.25 {
                out.push(format!("{what}: frame {}: \"{name}\" went back, {} → {} (on its way {} → {rest})", f + 1, pair[0], pair[1], ys[0]));
            }
            if way == 0. && (pair[1] - pair[0]).abs() > 0.25 {
                out.push(format!("{what}: frame {}: \"{name}\" moved, {} → {}, though it rests where it stood", f + 1, pair[0], pair[1]));
            }
        }
        if dropped && !lands_home {
            for (f, y) in ys.iter().enumerate().skip(1) {
                if (y - d.started_y).abs() < 1. && (rest - d.started_y).abs() > 1. {
                    out.push(format!("{what}: frame {f}: \"{name}\" is back where it started ({y})"));
                }
            }
        }
    }
    out
}

/// The middle of the first match for `target`.
pub(crate) fn middle(app: &mut TestApp, target: &str) -> (f32, f32) {
    let id = app.target(target).unwrap_or_else(|e| panic!("{e}"));
    let b = app.ui.scene.borrow().hit_bounds(id);
    (b.x + b.width / 2., b.y + b.height / 2.)
}

/// The engine's answers mid-motion: a session's state moving at frame 2
/// (the places as the engine still has them), its echo at frame 5.
fn engine(app: &mut TestApp, frame: usize) {
    match frame {
        2 => assert!(app.host.run_command("tick")),
        5 => assert!(app.host.run_command("echo")),
        _ => {}
    }
}

#[test]
fn a_dropped_session_lands_in_one_motion() {
    let mut app = mount();
    assert!(app.host.run_command("hold_echo"));
    let mut faulty = Vec::new();
    let pin = |app: &mut TestApp| middle(app, "#shell-sidebar >> \"Drop here to pin\"");

    let d = drop_at(&mut app, "Write the docs", pin, 40, engine);
    faulty.extend(faults("into Pinned", &d, false));

    let d = drop_at(&mut app, "Write the docs", |app| middle(app, "#shell-sidebar >> role:button \"Tidy the tests\""), 40, engine);
    faulty.extend(faults("out of Pinned", &d, false));

    // From a section of yours into Pinned.
    assert!(app.host.run_command("section"));
    app.frames(30);
    let d = drop_at(&mut app, "Answer the review", pin, 40, engine);
    faulty.extend(faults("from a section into Pinned", &d, false));

    // Let go where it was: back to its own place.
    let home = |app: &mut TestApp| {
        let b = app.bounds("#shell-sidebar >> role:button \"Ship the release\"");
        (b.x + 40., b.y + b.height / 2.)
    };
    let d = drop_at(&mut app, "Ship the release", home, 40, engine);
    faulty.extend(faults("back to its place", &d, true));

    assert!(faulty.is_empty(), "{}", faulty.join("\n"));
}

/// The sidebar's section headers keep their full height as it folds away
/// and comes back: nothing in it is squeezed or clipped on the way (a
/// header made again while the sidebar was away once grew from nothing as
/// it opened, its bottom half cut off).
#[test]
fn the_sidebars_headers_keep_their_height_through_a_toggle() {
    let mut app = mount();
    assert!(app.host.run_command("pin"));
    app.frames(30);
    let mut faults = Vec::new();
    for what in ["closing", "opening", "closing again", "opening again"] {
        assert!(app.host.run_command("shell.toggle_sidebar"));
        for f in 1..=24 {
            app.frame();
            let side = app.bounds("#shell-sidebar").width;
            // (Once it's narrower than a header's inset, it shows none of it.)
            if side < 16. {
                continue;
            }
            for head in ["Pinned", "Projects"] {
                let target = zgx_script::driver::parse(&format!("#shell-sidebar >> \"{head}\"")).unwrap();
                app.ui.prepare_frame();
                let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
                let Some(&id) = found.first() else {
                    faults.push(format!("{what}, frame {f}: no \"{head}\" (sidebar {side})"));
                    continue;
                };
                let scene = app.ui.scene.borrow();
                let full = scene.transformed_bounds(id).height;
                let shows = scene.visible_bounds(id).map_or(0., |r| r.height);
                if (full - shows).abs() > 0.5 {
                    faults.push(format!("{what}, frame {f}: \"{head}\" shows {shows:.1} of {full:.1} (sidebar {side:.1})"));
                }
            }
        }
        app.frames(30);
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// Zeron's column motion (crates/ui `WidthTween::panel`: motion
/// `PANEL_RESIZE`, CSS `ease`, 180 ms, longer moves stretched by the square
/// root of the distance over the sidebar's 256 to at most 240 ms): where a
/// column is `ms` into a move from `from` to `to`.
fn zerons(from: f32, to: f32, ms: f32) -> f32 {
    let duration = 180. * ((to - from).abs() / 256.).sqrt().clamp(1., 240. / 180.);
    let t = (ms / duration).clamp(0., 1.);
    // cubic-bezier(0.25, 0.1, 0.25, 1): x(s) = t solved by bisection.
    let (x1, y1, x2, y2) = (0.25_f32, 0.1_f32, 0.25_f32, 1.0_f32);
    let at = |a: f32, b: f32, s: f32| 3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s;
    let (mut lo, mut hi) = (0_f32, 1_f32);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.;
        if at(x1, x2, mid) < t { lo = mid } else { hi = mid }
    }
    from + (to - from) * at(y1, y2, (lo + hi) / 2.)
}

/// The panel's motion (ds/motion `panel`, 280 ms, cubicBezier(.22, 1, .36, 1)):
/// where it is `ms` into a move from `from` to `to`.
fn panels(from: f32, to: f32, ms: f32) -> f32 {
    let t = (ms / 280.).clamp(0., 1.);
    let (x1, y1, x2, y2) = (0.22_f32, 1.0_f32, 0.36_f32, 1.0_f32);
    let at = |a: f32, b: f32, s: f32| 3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s;
    let (mut lo, mut hi) = (0_f32, 1_f32);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.;
        if at(x1, x2, mid) < t { lo = mid } else { hi = mid }
    }
    from + (to - from) * at(y1, y2, (lo + hi) / 2.)
}

/// The sidebar folding and coming back moves as Zeron's columns do, and a
/// session's panel opening and closing on its own motion: frame by frame on
/// each curve and duration (from the frame after the toggle, as the move
/// starts).
#[test]
fn the_columns_move_as_zerons_do() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    let mut faults = Vec::new();
    // (The move starts on the frame after the toggle, or, the animation
    // engine asleep as it's asked, up to two frames later: its first frame
    // after a wake moves nothing (zgx anime `engine.rs`, `last = None`),
    // and whether it was awake follows the executor's real time. So the
    // curve is checked from its first moving frame, which must come within
    // two frames of the toggle.)
    let mut check = |app: &mut TestApp, what: &str, cmd: &str, id: &str, curve: fn(f32, f32, f32) -> f32| {
        let from = app.bounds(id).width;
        assert!(app.host.run_command(cmd));
        let mut ws = Vec::new();
        for _ in 0..24 {
            app.frame();
            ws.push(app.bounds(id).width);
        }
        let to = *ws.last().unwrap();
        let Some(first) = ws.iter().position(|w| (w - from).abs() > 0.05) else {
            faults.push(format!("{what}: never moved ({from})"));
            return;
        };
        if first > 2 {
            faults.push(format!("{what}: moved only {} frames after the toggle", first + 1));
        }
        for (i, w) in ws.iter().enumerate().skip(first) {
            let want = curve(from, to, (i - first + 1) as f32 * 1000. / 60.);
            if (w - want).abs() > 1. {
                faults.push(format!("{what}: frame {} of its move: {w:.1}, its curve's {want:.1} ({from} → {to})", i - first + 1));
            }
        }
        app.frames(30);
    };
    check(&mut app, "the sidebar folding", "shell.toggle_sidebar", "#shell-sidebar", zerons);
    check(&mut app, "the sidebar coming back", "shell.toggle_sidebar", "#shell-sidebar", zerons);
    check(&mut app, "the panel opening", "shell.toggle_files", "#shell-files", panels);
    check(&mut app, "the panel closing", "shell.toggle_files", "#shell-files", panels);
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// The sidebar's view options (its ⋯ at its top right) keep their place
/// as it folds and comes back, its edge clipping them there: they never
/// slide under the title bar's controls or the window's buttons.
#[test]
fn the_sidebars_view_options_never_pass_under_the_title_bars_controls() {
    let mut app = mount();
    app.frames(30);
    let fwd = app.bounds("#titlebar >> role:button \"Forward\"");
    let controls_end = fwd.x + fwd.width;
    let target = zgx_script::driver::parse("role:button \"Sidebar view options\"").unwrap();
    let mut faults = Vec::new();
    let mut open_x: Option<f32> = None;
    for what in ["closing", "opening", "closing again", "opening again"] {
        assert!(app.host.run_command("shell.toggle_sidebar"));
        for f in 1..=20 {
            app.frame();
            app.ui.prepare_frame();
            let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
            let Some(&id) = found.first() else { continue };
            let scene = app.ui.scene.borrow();
            if !scene.layout_visible(id) {
                continue;
            }
            let x = scene.transformed_bounds(id).x;
            let x0 = *open_x.get_or_insert(x);
            // (It goes with the window's 5-point gap before the sidebar's card as that
            // closes, no further.)
            if (x - x0).abs() > 5.5 {
                faults.push(format!("{what}, frame {f}: the ⋯ moved, {x0} → {x}"));
            }
            if let Some(shows) = scene.visible_bounds(id)
                && shows.width > 0.
                && shows.x < controls_end
            {
                faults.push(format!("{what}, frame {f}: the ⋯ shows at {:.0}..{:.0}, under the controls (to {controls_end})", shows.x, shows.x + shows.width));
            }
        }
        app.frames(30);
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// Nothing slides under the title bar's Panel toggle on any frame as the panel
/// opens, closes, and closes from expanded: the chats' tabs on the top bar and
/// Expand stop short of it, and the panel's strip (its tabs, its +) stands in the
/// panel's card, under the bar, the panel's edge clipping it.
#[test]
fn the_panels_strip_never_passes_under_the_title_bars_toggle() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    app.frames(40);
    let bar = app.bounds("#titlebar >> role:button \"Panel\"");
    let (toggle, foot) = (bar.x, bar.y + bar.height);
    // (On the bar's line, short of the toggle; in the card, under it.)
    let beside = ["#session-tabs >> role:tab", "#panel-expand"];
    let under = ["#panel-strip >> role:tab", "#panel-strip >> role:button \"New Tab\""];
    let mut faults = Vec::new();
    let mut watch = |app: &mut TestApp, what: &str| {
        for f in 1..=20 {
            app.frame();
            for part in beside.iter().chain(&under) {
                let target = zgx_script::driver::parse(part).unwrap();
                app.ui.prepare_frame();
                let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
                let scene = app.ui.scene.borrow();
                for id in found {
                    if let Some(r) = scene.visible_bounds(id)
                        && r.width > 0.
                        && r.x + r.width > toggle + 0.5
                        && r.y < foot - 0.5
                    {
                        faults.push(format!("{what}, frame {f}: {part} shows at {:.0}..{:.0}, {:.0} down, under the toggle (from {toggle}, to {foot} down)", r.x, r.x + r.width, r.y));
                    }
                }
            }
        }
        app.frames(30);
    };
    assert!(app.host.run_command("shell.toggle_files"));
    watch(&mut app, "closing");
    assert!(app.host.run_command("shell.toggle_files"));
    watch(&mut app, "opening");
    // Expanded over the session, then closed.
    app.click("#panel-expand");
    app.frames(40);
    assert!(app.host.run_command("shell.toggle_files"));
    watch(&mut app, "closing from expanded");
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}
