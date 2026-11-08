//! The measurement contract, exercised with the real invisible text nodes.
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("ui/composer_metrics_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(800., 600.);
    app.frames(3);
    assert_eq!(app.error(), None);
    app
}

fn result(app: &TestApp) -> Value {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n == "fixture_metrics_result")
        .expect("fixture result")
        .1
}

fn number(value: &Value, key: &str) -> f64 {
    value
        .field(key)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no {key}: {value:?}"))
}

fn ready(app: &TestApp) -> Value {
    let r = result(app);
    assert_eq!(r.field("status"), Some(&Value::from("ready")), "{r:?}");
    let s = r.field("sample").expect("measured sample").clone();
    assert_eq!(number(&s, "revision"), number(&r, "revision"), "{r:?}");
    assert!(
        (number(&s, "width") - number(&r, "width")).abs() < 0.5,
        "{r:?}"
    );
    s
}

fn two_nodes(app: &mut TestApp) -> Vec<zgx::scene::NodeId> {
    let nodes = [".composer-metric-line", ".composer-metric-wrapped"]
        .iter()
        .map(|selector| app.find(selector).expect("measurement node"))
        .collect::<Vec<_>>();
    assert_ne!(nodes[0], nodes[1]);
    assert!(app.find(".composer-metric-line[2]").is_none());
    assert!(app.find(".composer-metric-wrapped[2]").is_none());
    nodes
}

#[test]
fn composer_metrics_publish_empty_text_as_a_real_sample_then_refresh_equal_size_edits() {
    let mut app = mount();
    let nodes = two_nodes(&mut app);
    let history = app
        .host
        .states()
        .into_iter()
        .find(|(n, _)| n == "fixture_metrics_history")
        .unwrap()
        .1;
    assert_eq!(
        history.list().first(),
        Some(&Value::from("pending")),
        "initial zeroes are not a measured empty draft"
    );
    let empty = ready(&app);
    assert_eq!(number(&empty, "unwrapped_width"), 0., "{empty:?}");
    assert!(number(&empty, "wrapped_height") >= number(&empty, "line_height"));
    let mut previous = empty;
    for text in ["abc", "cba"] {
        app.host
            .set_state("fixture_metrics_text", Value::from(text))
            .unwrap();
        assert_eq!(result(&app).field("status"), Some(&Value::from("stale")));
        app.frames(3);
        let sample = ready(&app);
        assert!(number(&sample, "unwrapped_width") > 0.);
        assert!(number(&sample, "revision") > number(&previous, "revision"));
        if text == "cba" {
            assert_eq!(
                number(&sample, "unwrapped_width"),
                number(&previous, "unwrapped_width")
            );
            assert_eq!(
                number(&sample, "wrapped_height"),
                number(&previous, "wrapped_height")
            );
        }
        assert_eq!(
            two_nodes(&mut app),
            nodes,
            "edits must retain the same two measuring nodes"
        );
        previous = sample;
    }
}

#[test]
fn composer_metrics_reject_unknown_width_and_coalesce_replaced_requests() {
    let app = mount();
    let before = ready(&app);
    app.host
        .set_state("fixture_metrics_width", Value::Num(0.))
        .unwrap();
    app.frames(3);
    let unknown = result(&app);
    assert_eq!(unknown.field("status"), Some(&Value::from("stale")));
    assert_eq!(
        unknown.field("sample"),
        Some(&before),
        "zero width cannot replace a valid sample"
    );
    for (width, text) in [
        (100., "old"),
        (250., "intermediate"),
        (180., "latest\nline\n"),
    ] {
        app.host
            .set_state("fixture_metrics_width", Value::Num(width))
            .unwrap();
        app.host
            .set_state("fixture_metrics_text", Value::from(text))
            .unwrap();
    }
    app.frames(3);
    let sample = ready(&app);
    assert_eq!(number(&sample, "width"), 180.);
    assert_eq!(
        number(&sample, "wrapped_height"),
        3. * number(&sample, "line_height")
    );
    let settled = result(&app);
    app.frames(30);
    assert_eq!(result(&app), settled, "idle must not publish new revisions");
}

#[test]
fn composer_metrics_keep_the_target_width_when_the_presented_parent_resizes() {
    let app = mount();
    app.host
        .set_state(
            "fixture_metrics_text",
            Value::from("words ".repeat(30).as_str()),
        )
        .unwrap();
    app.frames(3);
    let before = ready(&app);
    for width in [600., 400., 200., 500.] {
        app.host
            .set_state("fixture_metrics_viewport", Value::Num(width))
            .unwrap();
        app.frames(2);
        assert_eq!(
            ready(&app),
            before,
            "a transient viewport width must not change target metrics"
        );
    }
}

#[test]
fn composer_metrics_revalidate_restored_samples_and_typography_changes() {
    let mut app = mount();
    app.host
        .set_state("fixture_metrics_text", Value::from("some words"))
        .unwrap();
    app.frames(3);
    let before = ready(&app);
    app.host.reload().unwrap();
    assert_eq!(
        result(&app).field("status"),
        Some(&Value::from("stale")),
        "restored measures need a fresh layout"
    );
    app.frames(3);
    let restored = ready(&app);
    assert!(number(&restored, "revision") > number(&before, "revision"));
    two_nodes(&mut app);
    assert!(app.host.run_command("fixture.metrics.type"));
    app.frames(3);
    let changed = ready(&app);
    assert_eq!(number(&changed, "line_height"), 28.);
    assert!(number(&changed, "revision") > number(&restored, "revision"));
    assert!(number(&changed, "unwrapped_width") > number(&restored, "unwrapped_width"));
}

#[test]
fn composer_metrics_wrap_at_requested_width_and_preserve_a_final_empty_line() {
    let app = mount();
    app.host
        .set_state("fixture_metrics_text", Value::from("one\ntwo\n"))
        .unwrap();
    app.frames(3);
    let multiline = ready(&app);
    assert!(
        number(&multiline, "wrapped_height") >= 3. * number(&multiline, "line_height"),
        "{multiline:?}"
    );
    app.host
        .set_state(
            "fixture_metrics_text",
            Value::from("words ".repeat(30).as_str()),
        )
        .unwrap();
    app.frames(3);
    let wide = ready(&app);
    app.host
        .set_state("fixture_metrics_width", Value::Num(120.))
        .unwrap();
    app.frames(3);
    let narrow = ready(&app);
    assert_eq!(number(&narrow, "width"), 120.);
    assert!(number(&narrow, "wrapped_height") > number(&wide, "wrapped_height"));
    assert_eq!(
        number(&narrow, "unwrapped_width"),
        number(&wide, "unwrapped_width")
    );
}
