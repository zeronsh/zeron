use zgx_script::{ScriptHost, Value, testing::TestApp};

struct Controller {
    app: TestApp,
    frame: u32,
}

impl Controller {
    fn new() -> Self {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("ui/composer_transition_fixture.luau");
        let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
        app.ui.scene.borrow_mut().resize(800., 600.);
        let mut c = Self { app, frame: 0 };
        c.frames(30);
        c
    }

    fn set(&self, key: &str, value: Value) {
        self.app.host.set_state(key, value).unwrap();
    }

    fn height(&self, height: f64) {
        self.set("fixture_transition_height", Value::Num(height));
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame += 1;
            self.set(
                "fixture_transition_now",
                Value::Num(f64::from(self.frame) * 1000. / 60.),
            );
            self.app.frame();
            assert_eq!(self.app.error(), None);
        }
    }

    fn report(&self) -> Value {
        assert!(self.app.host.run_command("fixture.transition.inspect"));
        self.app
            .host
            .states()
            .into_iter()
            .find(|(n, _)| n == "fixture_transition_report")
            .unwrap()
            .1
    }
}

fn num(v: &Value, field: &str) -> f64 {
    v.field(field)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("missing {field}: {v:?}"))
}

#[test]
fn composer_transition_rules_cover_width_clip_headers_and_interruption() {
    let ui = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    let report = zgx_script::test_runner::run(&ui, Some("composer transition:"), |outcome| {
        if let Some(failure) = &outcome.failure {
            eprintln!("{}: {failure}", outcome.name);
        }
    })
    .unwrap();
    assert_eq!(report.outcomes.len(), 4);
    assert_eq!(report.failed(), 0, "{report:?}");
}

#[test]
fn composer_transition_retargets_from_the_presented_frame_and_ignores_equal_destinations() {
    let mut c = Controller::new();
    c.height(132.);
    c.height(22.); // Reversed before the first display frame: already home.
    assert_eq!(c.report().field("active"), Some(&Value::Bool(false)));
    c.height(132.);
    c.frames(3);
    let before = c.report();
    assert_eq!(before.field("active"), Some(&Value::Bool(true)));
    c.height(500.);
    let replaced = c.report();
    assert_eq!(
        replaced.field("origin"),
        before.field("frame"),
        "retarget must start at the presented snapshot"
    );
    assert_eq!(num(&replaced, "progress"), 0.);
    c.frames(3);
    let growing = c.report();
    c.height(900.); // Same capped geometry, different natural content height.
    let unchanged = c.report();
    assert_eq!(num(&unchanged, "progress"), num(&growing, "progress"));
    assert_eq!(unchanged.field("origin"), growing.field("origin"));
    c.frames(1);
    assert!(num(&c.report(), "progress") > num(&growing, "progress"));
    c.frames(30);
    let settled = c.report();
    assert_eq!(settled.field("active"), Some(&Value::Bool(false)));
    assert_eq!(settled.field("frame"), settled.field("destination"));
    assert!(
        !c.app.clock.wants_frame(),
        "completed controller must stop requesting frames"
    );
    c.height(22.);
    c.frames(3);
    let shrinking = c.report();
    assert_eq!(shrinking.field("active"), Some(&Value::Bool(true)));
    c.height(180.);
    assert_eq!(
        c.report().field("origin"),
        shrinking.field("frame"),
        "reversing a shrink also starts at the presented snapshot"
    );
    c.frames(30);
    assert_eq!(c.report().field("frame"), c.report().field("destination"));
    assert!(!c.app.clock.wants_frame());
}

#[test]
fn composer_transition_context_and_reduce_motion_snap_and_cancel_the_running_clip() {
    let mut c = Controller::new();
    c.height(132.);
    c.frames(3);
    c.set("fixture_transition_snap", Value::Num(1.));
    let snapped = c.report();
    assert_eq!(snapped.field("frame"), snapped.field("destination"));
    assert_eq!(snapped.field("active"), Some(&Value::Bool(false)));
    c.height(500.); // A measurement settling during the context's snap window.
    assert_eq!(c.report().field("frame"), c.report().field("destination"));
    c.frames(30);
    c.height(100.);
    c.frames(3);
    assert_eq!(c.report().field("active"), Some(&Value::Bool(true)));
    c.set("motion_reduce", Value::Bool(true));
    let reduced = c.report();
    assert_eq!(reduced.field("frame"), reduced.field("destination"));
    assert_eq!(reduced.field("active"), Some(&Value::Bool(false)));
    c.frames(30);
    assert_eq!(
        c.report(),
        reduced,
        "cancelled callbacks cannot revive the old transition"
    );
    assert!(!c.app.clock.wants_frame());
}

#[test]
fn composer_transition_unmount_freezes_progress_and_remount_owns_a_new_controller() {
    let mut c = Controller::new();
    c.height(500.);
    c.frames(3);
    let before = c.report();
    c.set("fixture_transition_shown", Value::Bool(false));
    let disposed = c.report();
    assert_eq!(disposed.field("active"), Some(&Value::Bool(false)));
    assert_eq!(num(&disposed, "progress"), num(&before, "progress"));
    c.height(100.);
    c.frames(30);
    assert_eq!(
        c.report(),
        disposed,
        "unmounted controller cannot keep updating"
    );
    assert!(!c.app.clock.wants_frame());
    c.set("fixture_transition_shown", Value::Bool(true));
    let new = c.report();
    assert_eq!(num(&new, "progress"), 1.);
    assert_eq!(new.field("frame"), new.field("destination"));
    c.height(180.);
    c.frames(3);
    assert_eq!(c.report().field("active"), Some(&Value::Bool(true)));
    c.frames(30);
    assert_eq!(c.report().field("frame"), c.report().field("destination"));
}
