//! Syntax highlighting lexing checks (`ui/demos/syntax_tests.luau`), run
//! headless through the gallery's Syntax highlighting page so every language
//! (including Zig and Odin) is verified in zgx's Luau runtime.

use std::path::PathBuf;

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

#[test]
fn syntax_lexing_checks_pass() {
    let mut gallery = TestApp::new(|runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("gallery.luau"));
        host.action("appearance", |_| {});
        host.async_action("appearance", |_| async { Ok(Value::Nil) });
        host
    });
    gallery.ui.scene.borrow_mut().resize(1400., 2800.);
    gallery.frames(30);
    gallery.click(r#"role:button "Syntax highlighting"[1]"#);
    gallery.frames(20);
    assert_eq!(gallery.error(), None, "Syntax highlighting page errored");

    let tree = gallery.tree();
    for line in tree.lines() {
        if line.contains("lexing checks") {
            eprintln!("syntax verdict: {}", line.trim());
        }
    }
    assert!(
        tree.contains("lexing checks pass") && !tree.contains("lexing checks fail"),
        "syntax_tests.luau reported failures:\n{tree}"
    );
}
