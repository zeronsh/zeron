//! Settings → Archived with thousands of sessions (ui/screens/settings/archived,
//! `ds.Settings.EachList`): rows built only as the page shows them, which the
//! Lua test kit checks (ui/tests/settings_archived). What it can't: a hot
//! reload puts the page back where it stood, the same rows in the same
//! places; and a row's wash under the pointer, read off the scene, runs edge
//! to edge inside the card's own edge.

use std::path::PathBuf;

use zgx::scene::{NodeId, NodeKind, Rect};
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The shell on Settings → Archived, two thousand sessions shown.
fn archived() -> TestApp {
    let mut app = TestApp::new(|runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("shell_fixture.luau"));
        host.window_size(runtime.signal((1200., 900.)));
        host
    });
    app.ui.scene.borrow_mut().resize(1200., 900.);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed to load");
    assert!(app.host.run_command("archive_thousands"));
    assert!(app.host.run_command("shell.settings"));
    app.frames(10);
    app.click("#settings-nav >> \"Archived\"");
    app.frames(30);
    app.host
        .set_state("settings_archived_shown", Value::Num(5000.))
        .unwrap();
    app.frames(30);
    app
}

/// Each row of `Chat <n>` built: its words and where it stands.
fn rows(app: &mut TestApp) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for line in app.tree().lines() {
        let line = line.trim_start();
        let Some(at) = line.find("\"Chat ") else {
            continue;
        };
        // `@12 "Chat 3" 409,223 638x60`: a row, not its words inside it.
        if !line[..at]
            .trim_end()
            .ends_with(|c: char| c.is_ascii_digit())
        {
            continue;
        }
        let rest = &line[at + 1..];
        let Some(end) = rest.find('"') else { continue };
        let y = rest[end + 1..]
            .trim()
            .split(' ')
            .next()
            .and_then(|p| p.split(',').nth(1))
            .and_then(|v| v.parse().ok());
        if let Some(y) = y {
            out.push((rest[..end].to_string(), y));
        }
    }
    out
}

#[test]
fn a_reload_puts_the_page_back_with_the_same_rows_in_the_same_places() {
    let mut app = archived();
    app.host
        .set_state("shell_settings_scroll", Value::Num(60000.))
        .unwrap();
    app.frames(10);
    let before = rows(&mut app);
    assert!(
        !before.is_empty() && before.len() < 30,
        "only the rows it shows: {before:?}"
    );
    assert!(
        before.iter().all(|(words, _)| words != "Chat 1"),
        "far from the first: {before:?}"
    );
    app.host.reload().expect("the reload");
    app.frames(30);
    assert_eq!(app.error(), None);
    assert_eq!(rows(&mut app), before, "the same rows, where they stood");
}

/// The box a row's wash is painted on, under the pointer or not: the nearest
/// box around `row` with a fill, and that fill's alpha.
fn wash(app: &TestApp, row: NodeId) -> Option<(Rect, u8)> {
    let scene = app.ui.scene.borrow();
    let mut at = scene.parent(row);
    while let Some(id) = at {
        if let NodeKind::Quad(quad) | NodeKind::Panel { quad, .. } = scene.kind(id)
            && quad.fill.3 > 0
        {
            return Some((scene.bounds(id), quad.fill.3));
        }
        at = scene.parent(id);
    }
    None
}

#[test]
fn a_rows_wash_runs_edge_to_edge_in_its_card() {
    let mut app = archived();
    app.host
        .set_state("shell_settings_scroll", Value::Num(3000.))
        .unwrap();
    app.frames(10);
    let (words, _) = rows(&mut app).into_iter().nth(3).expect("rows in view");
    let target = format!("#settings-page >> role:group \"{words}\"");
    let row = app.find(&target).expect("the row");
    let bounds = app.ui.scene.borrow().bounds(row);
    let (card, _) = wash(&app, row).expect("the card's fill around it, at rest");
    assert_eq!(
        (card.x + 1., card.width - 2.),
        (bounds.x, bounds.width),
        "at rest the only fill is the card's, 1 pt of edge either side of the row"
    );
    app.hover(&target);
    app.frames(5);
    let row = app.find(&target).expect("the row");
    let (washed, _) = wash(&app, row).expect("its wash");
    assert_ne!(
        washed.height, card.height,
        "under the pointer the fill is the row's own, not the card's"
    );
    assert_eq!(
        (washed.x, washed.width),
        (bounds.x, bounds.width),
        "edge to edge: no inset, the card's width less its edge"
    );
}
