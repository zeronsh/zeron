//! Every menu in the app follows the design system's rules (README rules 5
//! and 6): a row's wash is concentric with its panel (the panel's radius
//! less its edge and the inset between them), and the room round the rows
//! is even, the same on all four sides. Measured off the scene, mounted
//! headless on each screen's fixture: the panel's and each row's painted
//! corner, and where the rows stand inside the panel.

use zgx::scene::{NodeId, NodeKind, Rect};
use zgx_script::{ScriptHost, testing::TestApp};

/// A box's painted corner and edge, if it paints one.
fn corner(app: &TestApp, id: NodeId) -> Option<(f32, f32)> {
    match app.ui.scene.borrow().kind(id) {
        NodeKind::Panel { quad, .. } | NodeKind::Quad(quad) => Some((quad.radius, quad.border_width)),
        _ => None,
    }
}

/// A menu's geometry: its panel's corner and edge, the room between its
/// edge and what it holds on each side (left, top, right, bottom), its
/// rounded rows (each one's corner and the corner concentric with the
/// panel at its inset), and the vertical gaps between adjacent stacked rows.
#[derive(Debug)]
pub struct Geometry {
    pub radius: f32,
    pub border: f32,
    pub inset: [f32; 4],
    pub rows: Vec<(f32, f32)>,
    pub gaps: Vec<f32>,
}

impl Geometry {
    /// What breaks the rules, if anything.
    pub fn faults(&self, what: &str) -> Vec<String> {
        let mut out = Vec::new();
        let [l, t, r, b] = self.inset;
        if (l - t).abs() > 0.5 || (l - r).abs() > 0.5 || (l - b).abs() > 0.5 {
            out.push(format!("{what}: its inset isn't even (left {l}, top {t}, right {r}, bottom {b})"));
        }
        for (corner, concentric) in &self.rows {
            if (corner - concentric).abs() > 0.5 {
                out.push(format!(
                    "{what}: a row's corner is {corner}, not {concentric} (the panel's {} less its edge {} and inset)",
                    self.radius, self.border
                ));
            }
        }
        for &gap in &self.gaps {
            if gap < 1.5 {
                out.push(format!(
                    "{what}: adjacent rows touch (gap {gap:.1}, need at least 2 from $zeron.list.gap)"
                ));
            }
        }
        out
    }
}

/// The menu open now, found by one of its items (`item`): its panel (the
/// nearest painted box round it) and what that holds.
pub fn measure(app: &mut TestApp, item: &str) -> Geometry {
    let target = zgx_script::driver::parse(item).expect("a target");
    let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
    assert!(!found.is_empty(), "no menu open ({item})");
    let scene = app.ui.scene.borrow();
    let mut panel = scene.parent(found[0]);
    while let Some(id) = panel {
        if corner(app, id).is_some_and(|(r, b)| r > 0. && b > 0.) {
            break;
        }
        panel = scene.parent(id);
    }
    let panel = panel.expect("a painted panel round the menu");
    let (radius, border) = corner(app, panel).expect("its corner");
    let p = scene.bounds(panel);
    let inner = p.width - border * 2.;
    // What it holds, in view: every box inside it with room, but those
    // edge to edge (its rules, its groups' boxes), whose own contents
    // count instead.
    let mut held: Vec<Rect> = Vec::new();
    let mut candidates: Vec<(Rect, f32)> = Vec::new();
    let mut stack = vec![panel];
    while let Some(id) = stack.pop() {
        for &child in scene.children(id) {
            let b = scene.bounds(child);
            let inside = b.width > 0.5
                && b.height > 0.5
                && b.x >= p.x - 0.5
                && b.y >= p.y - 0.5
                && b.x + b.width <= p.x + p.width + 0.5
                && b.y + b.height <= p.y + p.height + 0.5;
            if !inside || !scene.layout_visible(child) {
                continue;
            }
            if b.width < inner - 0.5 {
                held.push(b);
                if let Some((r, _)) = corner(app, child).filter(|c| c.0 > 0.) {
                    candidates.push((b, r));
                }
            }
            stack.push(child);
        }
    }
    let left = held.iter().map(|b| b.x).fold(f32::MAX, f32::min);
    let right = held.iter().map(|b| b.x + b.width).fold(f32::MIN, f32::max);
    let top = held.iter().map(|b| b.y).fold(f32::MAX, f32::min);
    let bottom = held.iter().map(|b| b.y + b.height).fold(f32::MIN, f32::max);
    let inset = [
        left - p.x - border,
        top - p.y - border,
        p.x + p.width - border - right,
        p.y + p.height - border - bottom,
    ];
    // Its rows: what follows the panel's edge, as wide as what it holds (a
    // control standing inside a row, a switch's pill, keeps its own corner).
    let full_rows: Vec<(Rect, f32)> = candidates
        .into_iter()
        .filter(|(b, _)| b.width >= right - left - 0.5)
        .collect();
    let rows = full_rows
        .iter()
        .map(|(b, r)| (*r, (radius - border - (b.x - p.x - border)).max(0.)))
        .collect();
    // Interactive rows inside the panel (including unhovered rows whose rest
    // background is transparent): any menuitem, option, checkbox, radio, or
    // full-width button descending from `panel`.
    let mut boxes: Vec<Rect> = full_rows.into_iter().map(|(b, _)| b).collect();
    for sel in ["role:menuitem", "role:option", "role:checkbox", "role:radio", "role:button"] {
        let q = zgx_script::driver::parse(sel).expect("role selector");
        for id in zgx_script::driver::find_all(&app.ui, &app.handle, &q).unwrap_or_default() {
            let mut cur = scene.parent(id);
            let mut in_panel = false;
            while let Some(pid) = cur {
                if pid == panel {
                    in_panel = true;
                    break;
                }
                cur = scene.parent(pid);
            }
            if !in_panel || !scene.layout_visible(id) {
                continue;
            }
            let b = scene.bounds(id);
            if b.width >= right - left - 0.5 && b.height > 0.5 {
                boxes.push(b);
            }
        }
    }
    // Vertical gaps between distinct stacked rows (deduplicating nested
    // full-width layers painted inside the same row).
    boxes.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.height.partial_cmp(&a.height).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut stacked: Vec<Rect> = Vec::new();
    for b in boxes {
        if let Some(prev) = stacked.last() {
            if b.y >= prev.y - 0.5 && b.y + b.height <= prev.y + prev.height + 0.5 {
                continue;
            }
        }
        stacked.push(b);
    }
    let gaps = stacked
        .windows(2)
        .map(|pair| pair[1].y - (pair[0].y + pair[0].height))
        .collect();
    Geometry { radius, border, inset, rows, gaps }
}

/// Until `target` shows (a folder read, a file opened: a host's answers
/// come a beat later).
pub fn wait(app: &mut TestApp, target: &str) {
    let target = target.to_owned();
    assert!(app.wait_until(|app| app.find(&target).is_some()), "{target} never showed");
}

fn files() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/files_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    // (As wide as leaves the file's actions in its "…" menu.)
    app.ui.scene.borrow_mut().resize(900., 600.);
    app.frames(30);
    wait(&mut app, "role:treeitem \"long.txt\"");
    app
}

fn shell() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    app
}

fn composer_place() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/composer_place_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1000., 600.);
    app.frames(10);
    app.host
        .set_state(
            "test_place_devices",
            zgx_script::Value::from_json(
                r#"[{"value":"work-laptop","label":"work-laptop","you":true},{"value":"home-laptop","label":"home-laptop"}]"#,
            )
            .expect("valid devices json"),
        )
        .expect("set test_place_devices");
    app.host
        .set_state(
            "test_place_device",
            zgx_script::Value::from_json(r#""work-laptop""#).expect("valid device json"),
        )
        .expect("set test_place_device");
    app.frames(20);
    app
}

/// Opens a menu (`open`), measures it by one of its items, and closes it.
fn check(app: &mut TestApp, what: &str, open: impl FnOnce(&mut TestApp), item: &str, faults: &mut Vec<String>) {
    open(app);
    // (A menu may open a beat later: on the clock, or once what it lists
    // has come.)
    let target = item.to_owned();
    app.wait_until(|app| app.find(&target).is_some());
    app.frames(30);
    let g = measure(app, item);
    faults.extend(g.faults(what));
    app.key(zgx::input::Key::Escape);
    app.frames(30);
}

#[test]
fn every_menu_is_concentric_and_evenly_inset() {
    let mut faults = Vec::new();

    let mut app = files();
    app.click("role:treeitem \"long.txt\"");
    wait(&mut app, "role:button \"More\"");
    app.frames(30);
    check(&mut app, "the file's \"…\" menu", |app| app.click("role:button \"More\""), "role:menuitem \"Copy Path\"", &mut faults);
    check(
        &mut app,
        "the file tree's row menu",
        |app| {
            app.click("role:treeitem \"long.txt\"");
            app.frames(5);
            app.keys("shift+F10");
        },
        "role:menuitem",
        &mut faults,
    );

    let mut app = shell();
    check(
        &mut app,
        "the session menu",
        |app| {
            crate::focus_tests::keys_on_row(app, "#shell-sidebar >> role:button \"Fix the build\"");
            app.keys("shift+F10");
        },
        "role:menuitem \"Rename\"",
        &mut faults,
    );
    check(&mut app, "the sidebar's view options", |app| app.click("role:button \"Sidebar view options\""), "role:checkbox \"By project\"", &mut faults);
    check(&mut app, "the composer's access picker", |app| app.click("role:button \"Access\""), "role:option", &mut faults);
    check(&mut app, "the composer's model picker", |app| app.click("role:button \"Model\""), "role:option", &mut faults);

    let mut app = composer_place();
    check(
        &mut app,
        "the composer's Checkout submenu",
        |app| {
            app.click("role:button \"Branch\"");
            app.frames(20);
            app.click("role:menuitem \"Checkout\"");
        },
        "role:listbox \"Checkout\" >> role:option",
        &mut faults,
    );
    app.key(zgx::input::Key::Escape);
    app.frames(20);
    // Searchable dropdowns whose top is the centered 36px search field or
    // whose bottom is a virtualized `ui.list` (3px py + 1px row wrapper py)
    // still obey concentric row corners and `$zeron.list.gap` between rows.
    for (what, open_sub, item) in [
        ("the composer's Work in picker", None, "role:menuitem \"Device\""),
        ("the composer's Device submenu", Some("role:menuitem \"Device\""), "\"Search devices…\""),
    ] {
        app.click("role:button \"Branch\"");
        app.frames(20);
        if let Some(sub) = open_sub {
            app.click(sub);
            app.frames(20);
        }
        let g = measure(&mut app, item);
        for (corner, concentric) in &g.rows {
            if (corner - concentric).abs() > 0.5 {
                faults.push(format!("{what}: a row's corner is {corner}, not {concentric}"));
            }
        }
        for &gap in &g.gaps {
            if gap < 1.5 {
                faults.push(format!("{what}: adjacent rows touch (gap {gap:.1}, need at least 2 from $zeron.list.gap)"));
            }
        }
        app.key(zgx::input::Key::Escape);
        app.frames(20);
        if open_sub.is_some() {
            app.key(zgx::input::Key::Escape);
            app.frames(20);
        }
    }

    assert!(faults.is_empty(), "{}", faults.join("\n"));
}
