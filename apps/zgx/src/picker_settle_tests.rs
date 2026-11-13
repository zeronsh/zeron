//! The model picker stops waiting on a device that doesn't answer: while
//! another device's catalog is looked at, its label is a breathing
//! skeleton (ds/skeleton, a `ui.loop`), and an offline device's look never
//! answers. The host settles it (src/chat.rs `settle_remote`); settled, the
//! skeleton goes and the window rests, no frames asked for. Mounted headless
//! on the chat fixture, the open session moved to the box.

use zgx_script::{ScriptHost, testing::TestApp};

fn on_the_box() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    assert!(app.host.run_command("session_on_box"));
    app.frames(30);
    app
}

#[test]
fn a_device_that_doesnt_answer_leaves_the_picker_at_rest_once_settled() {
    let mut app = on_the_box();
    assert!(app.host.run_command("catalog_box_looking"));
    app.frames(60);
    assert!(app.clock.wants_frame(), "looking: the picker's skeleton breathes");
    assert!(app.host.run_command("catalog_box_settled"));
    app.frames(60);
    assert_eq!(app.error(), None);
    assert!(!app.clock.wants_frame(), "settled: nothing breathes, no frames asked for");
    assert!(app.target("role:button \"Model\"").is_ok(), "the model chip still there");
}
