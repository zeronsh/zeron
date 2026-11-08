//! Pure geometry rules run in Luau, where the production calculation lives.
use zgx_script::{ScriptHost, Value, testing::TestApp};

#[test]
fn composer_geometry_rules_have_boundary_header_and_purity_coverage() {
    let ui = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    let report = zgx_script::test_runner::run(&ui, Some("composer geometry:"), |outcome| {
        if let Some(failure) = &outcome.failure {
            eprintln!("{}: {failure}", outcome.name);
        }
    })
    .expect("geometry tests load");
    assert_eq!(report.outcomes.len(), 8, "all geometry cases must run");
    assert_eq!(report.failed(), 0, "{report:?}");
}

#[test]
fn composer_headers_grow_and_shrink_without_replacing_or_covering_the_editor() {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/composer_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(700., 600.);
    app.frames(30);
    app.fill("role:textbox", "keep this draft");
    app.frames(30);
    let field = app.find("role:textbox").unwrap();
    let editor = app.ui.editor_within(field).unwrap();
    editor.set_selection(1, 8);
    let short = app.bounds("role:textbox");
    for (state, value, header) in [
        (
            "test_composer_cargo",
            r#"[{"id":"f1","kind":"file","name":"notes.md"}]"#,
            "notes.md",
        ),
        (
            "test_composer_question",
            r#"{"text":"Choose a branch","options":[{"value":"main","label":"Main"}]}"#,
            "Choose a branch",
        ),
    ] {
        app.host
            .set_state(state, Value::from_json(value).unwrap())
            .unwrap();
        for _ in 0..30 {
            app.frames(1);
            let current = app.find("role:textbox").unwrap();
            let handle = app.ui.editor_within(current).unwrap();
            assert_eq!(handle.node, editor.node);
            assert_eq!(handle.selection(), (1, 8));
            assert_eq!(app.ui.input.focused(), Some(editor.node));
            assert_eq!(handle.editor.borrow().text().as_ref(), "keep this draft");
            assert_eq!(app.error(), None);
        }
        let field_box = app.bounds("role:textbox");
        let role = if state.ends_with("cargo") {
            "button"
        } else {
            "radiogroup"
        };
        let head = app
            .find(&format!("role:{role} \"{header}\""))
            .unwrap_or_else(|| panic!("{state}: missing {header}\n{}", app.tree()));
        let head_box = app.ui.scene.borrow().transformed_bounds(head);
        assert!(
            field_box.height > short.height,
            "header opens the expanded editor"
        );
        assert!(
            head_box.y + head_box.height <= field_box.y,
            "header overlaps the text: {head_box:?}, {field_box:?}"
        );
        app.host
            .set_state(
                state,
                if state.ends_with("cargo") {
                    Value::List(Vec::new().into())
                } else {
                    Value::Nil
                },
            )
            .unwrap();
        app.frames(40);
        let back = app.bounds("role:textbox");
        assert!(
            (back.y - short.y).abs() < 1. && (back.height - short.height).abs() < 1.,
            "removing header restores the compact draft"
        );
    }
}
