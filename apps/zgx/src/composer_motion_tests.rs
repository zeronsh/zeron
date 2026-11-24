use zgx_script::{ScriptHost, Value, testing::TestApp};

struct Controller {
    app: TestApp,
}

impl Controller {
    fn new() -> Self {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("ui/composer_transition_fixture.luau");
        let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
        app.ui.scene.borrow_mut().resize(800., 600.);
        let mut c = Self { app };
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
    assert_eq!(report.outcomes.len(), 7);
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
    c.set("fixture_transition_ready", Value::Bool(false));
    c.set("fixture_transition_snap", Value::Num(1.));
    let snapped = c.report();
    assert_eq!(snapped.field("frame"), snapped.field("destination"));
    assert_eq!(snapped.field("active"), Some(&Value::Bool(false)));
    c.frames(100); // Readiness is independent of how long the machine takes.
    assert_eq!(c.report().field("settling"), Some(&Value::Bool(true)));
    c.height(500.); // A late measurement still belongs to the same context.
    assert_eq!(c.report().field("frame"), c.report().field("destination"));
    c.set("fixture_transition_revision", Value::Num(2.));
    c.set("fixture_transition_ready", Value::Bool(true));
    c.frames(2);
    assert_eq!(c.report().field("settling"), Some(&Value::Bool(false)));
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

#[test]
fn composer_transition_replaced_context_cannot_acknowledge_a_pending_revision() {
    let mut c = Controller::new();
    c.set("fixture_transition_snap", Value::Num(1.));
    // Replace the context before its deferred acknowledgement can run.
    c.set("fixture_transition_ready", Value::Bool(false));
    c.set("fixture_transition_revision", Value::Num(2.));
    c.set("fixture_transition_snap", Value::Num(2.));
    c.height(500.);
    c.frames(40);
    assert_eq!(c.report().field("settling"), Some(&Value::Bool(true)));
    assert_eq!(c.report().field("frame"), c.report().field("destination"));
    c.set("fixture_transition_ready", Value::Bool(true));
    c.frames(2);
    assert_eq!(c.report().field("settling"), Some(&Value::Bool(false)));
    assert!(!c.app.clock.wants_frame());
}

#[test]
fn composer_width_only_resize_publishes_the_frame_without_animating_and_preserves_height_motion() {
    let mut c = Controller::new();
    for height in [22., 132.] {
        c.height(height);
        c.frames(30);
        for width in [620., 580., 660.] {
            c.set("fixture_transition_width", Value::Num(width));
            let report = c.report();
            assert_eq!(report.field("active"), Some(&Value::Bool(false)));
            assert_eq!(report.field("frame"), report.field("destination"));
            assert_eq!(num(report.field("frame").unwrap(), "width"), width);
            assert!(!c.app.clock.wants_frame());
        }
    }
    c.height(500.);
    c.frames(3);
    let shown = c.report();
    c.set("fixture_transition_width", Value::Num(600.));
    let resized = c.report();
    assert_eq!(resized.field("active"), Some(&Value::Bool(true)));
    assert_eq!(resized.field("origin"), shown.field("frame"));
    c.frames(30);
    assert_eq!(c.report().field("frame"), c.report().field("destination"));
    assert!(!c.app.clock.wants_frame());
}

#[test]
fn removing_last_attachment_to_skinny_avoids_transcript_repaint_storm() {
    use std::path::PathBuf;
    use zgx::input::InputEvent;

    let ui_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    let build_app = || -> TestApp {
        let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir.join("chat_fixture.luau")));
        app.ui.scene.borrow_mut().resize(1240., 820.);
        app.frames(2);
        assert!(app.host.run_command("go_s6"));
        app.frames(30);
        // Populate a rich multi-message transcript so any accidental per-frame
        // transcript layout/repaint shows up unmistakably in damage area.
        let mut rows = Vec::new();
        for i in 0..8 {
            let text = format!(
                "## Section {i}\n\nAssistant reply with **bold**, `inline code`, and a code block:\n\n```rust\nfn handle_frame_{i}(dt: f32) -> f32 {{\n    let eased = 1.0 - (1.0 - dt).powi(3);\n    eased * 100.0\n}}\n```\n\n- Bullet one explaining layout geometry\n- Bullet two explaining scroll anchoring and bottom fade\n"
            );
            let user_entry = serde_json::json!({
                "id": format!("u{i}"),
                "role": "user",
                "parts": [{ "id": format!("u{i}p"), "kind": "text", "text": format!("Question {i}: check composer shrink.") }],
                "createdAt": 1_762_000_000_000_i64 + i as i64 * 1000
            });
            let asst_entry = serde_json::json!({
                "id": format!("a{i}"),
                "role": "assistant",
                "status": "complete",
                "parts": [{ "id": format!("a{i}x"), "kind": "text", "text": text }],
                "createdAt": 1_762_000_000_500_i64 + i as i64 * 1000
            });
            rows.push(crate::chat::built_entry(&user_entry, "doc", None, Some("Opus")).unwrap());
            rows.push(crate::chat::built_entry(&asst_entry, "doc", None, Some("Opus")).unwrap());
        }
        app.host
            .shared("chat.messages", Value::Nil)
            .set(Value::List(rows.into()));
        app.frames(30);
        app
    };

    let drop_path = |app: &mut TestApp, path: PathBuf| {
        let r = app.bounds("#transcript");
        app.ui.dispatch(InputEvent::FilesDrop {
            x: r.x + r.width / 2.,
            y: r.y + r.height * 0.3,
            paths: vec![path].into(),
        });
        app.frames(30);
    };

    let click_remove_chip = |app: &mut TestApp| {
        let chip = app
            .find(r#"#chat-composer >> role:button "notes.md""#)
            .or_else(|| app.find(r#"#chat-composer >> role:button ~"zgx""#))
            .or_else(|| app.find(r#"#chat-composer >> role:button ~"Image""#))
            .expect("cargo chip button");
        let r = app.ui.scene.borrow().transformed_bounds(chip);
        app.ui.dispatch(InputEvent::PointerMove {
            x: r.x + r.width / 2.,
            y: r.y + r.height / 2.,
        });
        app.frames(10);
        let _ = app.ui.scene.borrow_mut().flush();
        app.click(r#"#chat-composer >> role:button ~"Remove""#);
    };

    let verify_shrink = |label: &str, path: PathBuf| -> TestApp {
        let mut app = build_app();
        drop_path(&mut app, path);
        let expanded_h = app.bounds("#chat-composer").height;
        assert!(
            expanded_h > 180.0,
            "{label}: expected expanded composer with cargo + inline chip, got {expanded_h}"
        );
        let _ = app.ui.scene.borrow_mut().flush();

        click_remove_chip(&mut app);

        let mut prev_h = expanded_h;
        let mut prev_tb_x = app.bounds("role:textbox \"Message\"").x;
        let mut prev_tb_y = app.bounds("role:textbox \"Message\"").y;
        let mut anim_damage_total = 0.0f64;
        let mut max_anim_frame_damage = 0.0f32;
        let mut max_anim_calls = 0u64;

        for f in 0..16 {
            let calls_before = app.calls();
            app.frame();
            let report = app.ui.scene.borrow_mut().flush();
            let dcalls = app.calls() - calls_before;
            let dmg_pt2: f32 = report.damage.iter().map(|r| r.width * r.height).sum();
            let comp_h = app.bounds("#chat-composer").height;
            let tb = app.bounds("role:textbox \"Message\"");

            assert!(
                comp_h <= prev_h + 0.05,
                "{label} f{f:02}: composer height must shrink monotonically ({prev_h:.2} -> {comp_h:.2})"
            );
            assert!(
                tb.x >= prev_tb_x - 0.05,
                "{label} f{f:02}: textbox x must shift monotonically right ({prev_tb_x:.2} -> {:.2})",
                tb.x
            );
            assert!(
                tb.y >= prev_tb_y - 0.05,
                "{label} f{f:02}: textbox y must shift monotonically down ({prev_tb_y:.2} -> {:.2})",
                tb.y
            );
            prev_h = comp_h;
            prev_tb_x = tb.x;
            prev_tb_y = tb.y;

            if f >= 1 {
                anim_damage_total += dmg_pt2 as f64;
                max_anim_frame_damage = max_anim_frame_damage.max(dmg_pt2);
                max_anim_calls = max_anim_calls.max(dcalls);
            }
        }

        let settled_h = app.bounds("#chat-composer").height;
        assert!(
            (settled_h - 81.0).abs() < 2.0,
            "{label}: composer should settle to compact skinny height (~81), got {settled_h:.2}"
        );
        assert!(
            max_anim_frame_damage < 250_000.0,
            "{label}: animation frame repainted too much area ({max_anim_frame_damage:.0} pt2; full transcript repaint regression)"
        );
        assert!(
            anim_damage_total < 1_500_000.0,
            "{label}: total animation damage across f01..f15 too high ({anim_damage_total:.0} pt2)"
        );
        assert!(
            max_anim_calls <= 50,
            "{label}: animation frame script calls spiked to {max_anim_calls}"
        );
        assert_eq!(app.error(), None, "{label}: unexpected script error");
        app
    };

    let _file_app = verify_shrink("file", PathBuf::from("/tmp/zeron-drop/notes.md"));

    let img = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/assets/favicons/zgx.png");
    let mut img_app = verify_shrink("image", img.clone());

    // Re-attaching an image after 1 -> 0 must still open the Lightbox cleanly
    // (Lightbox.Host stays mounted on the composer root across 0 <-> 1 cargo).
    drop_path(&mut img_app, img);
    img_app.click(r#"#chat-composer >> role:button ~"zgx""#);
    img_app.frames(6);
    assert!(
        img_app.find(r#"role:dialog "Image viewer""#).is_some(),
        "image lightbox should open cleanly after removing and re-attaching an image"
    );
    assert_eq!(img_app.error(), None);
}

