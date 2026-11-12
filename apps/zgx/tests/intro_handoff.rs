//! The intro must keep its preloaded onboarding screen entirely hidden,
//! then reveal it and hand off once, including an early Escape.
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use zgx_script::{ScriptHost, testing::TestApp};

fn intro() -> (TestApp, Rc<RefCell<Vec<&'static str>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let log = events.clone();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/intro.luau"));
        host.shared("onboarding.prepare_hidden", true);
        host.shared("platform", "windows");
        for name in ["prepare", "interactive", "reveal", "done"] {
            let log = log.clone();
            host.action(name, move |_| log.borrow_mut().push(name));
        }
        host.action("appearance", |_| {});
        host
    });
    app.ui.scene.borrow_mut().resize(1280., 800.);
    (app, events)
}

#[test]
fn the_staged_screen_is_invisible_until_the_handoff() {
    let (mut app, events) = intro();
    app.frames(500);
    assert_eq!(app.error(), None);
    assert!(events.borrow().contains(&"prepare"));
    assert!(events.borrow().contains(&"interactive"));
    assert_eq!(app.opacity("#sheet"), 0.);
    app.click("#cta >> role:button");
    app.frames(10);
    assert_eq!(app.opacity("#sheet"), 1.);
    app.frames(160);
    let events = events.borrow();
    assert_eq!(events.iter().filter(|&&e| e == "prepare").count(), 1);
    assert_eq!(events.iter().filter(|&&e| e == "reveal").count(), 1);
    assert_eq!(events.iter().filter(|&&e| e == "done").count(), 1);
}

#[test]
fn escape_during_staging_still_hands_off_once() {
    let (mut app, events) = intro();
    app.frames(1);
    app.keys("Escape");
    app.keys("Escape");
    app.frames(170);
    assert_eq!(app.error(), None);
    let events = events.borrow();
    assert_eq!(events.iter().filter(|&&e| e == "prepare").count(), 1);
    assert_eq!(events.iter().filter(|&&e| e == "reveal").count(), 1);
    assert_eq!(events.iter().filter(|&&e| e == "done").count(), 1);
}
