//! Dictation uses the native editor, with deterministic transcription events.
use std::path::PathBuf;
use zgx_script::{ScriptHost, Value, testing::TestApp};
fn app() -> TestApp {
    let mut app = TestApp::new(|r| {
        let host = ScriptHost::new(
            r,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/dictation_fixture.luau"),
        );
        host.action("dictation.control", |_| {});
        host.action("dictation.guard", |_| {});
        host
    });
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(30);
    assert_eq!(app.error(), None);
    app.click("role:textbox \"Message\"");
    app.frames(2);
    app
}
fn state(app: &TestApp, key: &str) -> String {
    app.host
        .states()
        .into_iter()
        .find_map(|(k, v)| {
            if k == key {
                if let Value::Str(s) = v {
                    Some(s.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or_default()
}
fn command(app: &mut TestApp, key: &str) {
    assert!(app.host.run_command(key));
    app.frames(10);
    assert_eq!(app.error(), None);
}
#[test]
fn dictation_replaces_unicode_selection_as_one_undo_step() {
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "hola mundo");
    a.frames(3);
    a.ui.focused_editor().unwrap().set_selection(5, 10);
    a.frames(2);
    command(&mut a, "begin");
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "hola 你好 🌎");
    a.keys("cmd+z");
    a.frames(5);
    assert_eq!(state(&a, "dictation_draft"), "hola mundo");
}
#[test]
fn dictation_discards_stale_cancelled_and_failed_results() {
    for op in ["move", "cancel", "fail", "send_other"] {
        let mut a = app();
        a.type_text("role:textbox \"Message\"", "draft");
        a.frames(3);
        command(&mut a, "begin");
        command(&mut a, op);
        command(&mut a, "final");
        assert_eq!(state(&a, "dictation_draft"), "draft", "{op}");
        assert_eq!(state(&a, "dictation_sent"), "");
    }
    let mut a = app();
    command(&mut a, "begin");
    a.type_text("role:textbox \"Message\"", "edited");
    a.frames(3);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "edited");
}
#[test]
fn dictation_send_waits_for_final_and_empty_speech_never_sends() {
    let mut a = app();
    command(&mut a, "begin");
    a.keys("Enter");
    a.frames(3);
    assert_eq!(state(&a, "dictation_sent"), "");
    command(&mut a, "final");
    assert_eq!(
        state(&a, "dictation_sent"),
        "你好 🌎",
        "states: {:?} tree: {}",
        a.host.states(),
        a.tree()
    );
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "draft");
    a.frames(3);
    command(&mut a, "begin");
    a.keys("Enter");
    a.frames(3);
    command(&mut a, "empty");
    assert_eq!(state(&a, "dictation_sent"), "");
    assert_eq!(state(&a, "dictation_draft"), "draft");
}

#[test]
fn dictation_send_button_preserves_the_draft_until_finalization() {
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "draft ");
    a.frames(3);
    command(&mut a, "begin");
    a.click("role:button \"Send message\"");
    a.frames(3);
    assert_eq!(state(&a, "dictation_sent"), "");
    assert_eq!(state(&a, "dictation_draft"), "draft ");
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_sent"), "draft 你好 🌎");
}

#[test]
fn dictation_moving_the_selection_or_leaving_the_editor_invalidates_the_session() {
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "draft");
    a.frames(3);
    command(&mut a, "begin");
    a.keys("ArrowLeft");
    a.frames(2);
    a.keys("ArrowRight");
    a.frames(2);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "draft");
    command(&mut a, "begin");
    a.click("role:button \"Model\"");
    a.frames(3);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "draft");
    assert_eq!(state(&a, "dictation_sent"), "");
}

#[test]
fn dictation_can_finalize_a_queue_send_while_the_agent_is_running() {
    let mut a = app();
    a.host
        .set_state("dictation_running", Value::Bool(true))
        .unwrap();
    command(&mut a, "begin");
    assert!(a.find("role:button \"Queue message\"").is_some());
    a.keys("Enter");
    a.frames(3);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_sent"), "你好 🌎");
}
