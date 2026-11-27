use std::path::PathBuf;

use zgx::scene::{Color, NodeKind};
use zgx::theme::tok;
use zgx_script::{ScriptHost, testing::TestApp};

#[test]
fn back_to_chats_paints_the_hover_wash_only_under_the_pointer() {
    let ui = std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, ui.join("shell_fixture.luau")));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    assert!(app.host.run_command("shell.settings"));
    app.frames(30);

    let back = "#settings-nav >> role:button \"Back to chats\"";
    let search = "#settings-nav >> role:textbox \"Search settings\"";
    let row = app.target(back).expect("the back button in Settings");
    let fill = |app: &TestApp| match app.ui.scene.borrow().kind(row) {
        NodeKind::Panel { quad, .. } => quad.fill,
        NodeKind::Container(_) => Color(0, 0, 0, 0),
        other => panic!("expected a navigation row, got {other:?}"),
    };

    assert_eq!(fill(&app).3, 0);
    app.hover(back);
    assert_eq!(
        fill(&app),
        app.ui.theme().root().color_untracked(tok::HOVER)
    );
    app.hover(search);
    assert_eq!(fill(&app).3, 0);
}
