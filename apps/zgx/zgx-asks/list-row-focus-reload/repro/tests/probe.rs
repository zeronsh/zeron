//! A reload gives focus back to a plain focusable node, but not to a
//! `ui.list` row (zgx-asks/list-row-focus-reload). `cargo test -- --nocapture`
//! from this folder: the plain node passes, the row fails.
use zgx_script::{ScriptHost, testing::TestApp};

/// Clicks `target`, reloads, and says whether it was focused before and after.
fn focus_across_reload(target: &str) -> (bool, bool) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/app.luau");
    let mut app = TestApp::new(move |rt| ScriptHost::new(rt, path.clone()));
    app.ui.scene.borrow_mut().resize(800., 600.);
    app.frames(10);
    app.click(target);
    app.frames(10);
    let label = target.split('"').nth(1).unwrap().to_owned();
    let focused = |app: &mut TestApp| {
        app.tree().lines().find(|l| l.contains(&format!("\"{label}\""))).is_some_and(|l| l.contains("focused"))
    };
    let before = focused(&mut app);
    app.host.reload().expect("the reload failed");
    app.frames(30);
    (before, focused(&mut app))
}

#[test]
fn a_plain_node_keeps_focus_across_a_reload() {
    assert_eq!(focus_across_reload("role:button \"Plain\""), (true, true));
}

#[test]
fn a_list_row_keeps_focus_across_a_reload() {
    // Today: (true, false). The row is rebuilt under the same key and
    // nothing is focused after the reload.
    assert_eq!(focus_across_reload("role:treeitem \"r3\""), (true, true));
}
