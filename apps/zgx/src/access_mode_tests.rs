//! Changing agents must not leave an unsupported Auto mode looking usable.
use zgx_script::{ScriptHost, Value, testing::TestApp};

#[test]
fn unsupported_auto_is_labelled_unavailable_until_explicitly_changed() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/auto_access_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 600.);
    app.frames(30);
    assert!(app.texts().iter().any(|text| text == "Auto"));
    let offered = app.host.shared("test_composer_auto", Value::Bool(true));
    let access = app.host.shared("test_composer_access", Value::from("auto"));
    offered.set(Value::Bool(false));
    app.frames(30);
    // The animated label renders changing letters as separate text nodes.
    assert!(app.texts().concat().contains("Auto unavailable"));
    assert_eq!(
        access.get(),
        Value::from("auto"),
        "never silently grant full access"
    );
    app.click("role:button \"Access\"");
    app.frames(30);
    assert!(
        app.texts()
            .iter()
            .any(|text| text.contains("Choose another access mode or agent."))
    );
    app.click("role:option \"Full access Run commands and edits without asking.\"");
    app.frames(30);
    assert_eq!(access.get(), Value::from("full"));
    assert!(app.wait_until(|app| !app.texts().concat().contains("Auto unavailable")));
    assert_eq!(app.error(), None);
}
