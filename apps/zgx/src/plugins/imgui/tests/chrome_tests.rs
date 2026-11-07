use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::Value as Json;
use zgx_script::testing::TestApp;
use zgx_script::{Plugin, ScriptHost, Value};

use crate::plugins::vlist;

const ID: &str = "imgui-custom-chrome";
const WINDOW: &str = "chrome";
const TITLE: &str = "Custom Chrome";

type Heard = Rc<RefCell<Vec<String>>>;

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("plugins-dev")
        .join(ID)
}

fn manifest() -> Json {
    let text = std::fs::read_to_string(folder().join("zeron-plugin.json")).expect("its manifest");
    serde_json::from_str(&text).expect("a manifest")
}

fn harness() -> (TestApp, Heard) {
    let entry = folder().join("init.luau");
    let heard: Heard = Heard::default();
    let log = heard.clone();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        for (key, _) in crate::plugins::grants::SHARED_READ {
            host.expose_shared_read(*key, None);
            host.shared(key, Value::Nil);
        }
        for name in crate::plugins::grants::OWN_SHARED {
            host.expose_shared_read(format!("plugin.{ID}.{name}"), None);
            let empty = match *name {
                "calls" | "inbox" | "menu_items" | "locales" | "composed" => vlist(Vec::new()),
                _ => Value::record(std::iter::empty::<(&str, Value)>()),
            };
            host.shared(&format!("plugin.{ID}.{name}"), Value::Nil)
                .set(empty);
        }
        host.expose_action(format!("plugin.{ID}.imgui"), None);
        host.expose_action(format!("plugin.{ID}.imgui_end"), None);
        for name in crate::plugins::grants::OWN_ACTIONS {
            host.expose_action(format!("plugin.{ID}.{name}"), None);
        }
        let answered = log.clone();
        host.function(format!("plugin.{ID}.imgui"), move |arg| {
            let asked: Json = serde_json::from_str(&arg.to_json()).unwrap_or_default();
            answered.borrow_mut().push(asked.to_string());
            host_answer(&asked)
        });
        let ended = log.clone();
        host.action(format!("plugin.{ID}.imgui_end"), move |arg| {
            ended.borrow_mut().push(format!("end {}", arg.to_json()));
        });
        host.set_plugins(vec![Plugin {
            name: ID.into(),
            entry,
            grants: vec![],
        }]);
        host
    });
    (app, heard)
}

fn host_answer(asked: &Json) -> Value {
    match asked["op"].as_str().unwrap_or_default() {
        "open_window" => Value::record([
            ("ok", Value::Bool(true)),
            ("id", Value::from(asked["id"].as_str().unwrap_or_default())),
        ]),
        "is_maximized" => {
            Value::record([("ok", Value::Bool(true)), ("maximized", Value::Bool(false))])
        }
        "invisible_button" => Value::record([
            ("ok", Value::Bool(true)),
            ("hovered", Value::Bool(false)),
            ("active", Value::Bool(false)),
            ("clicked", Value::Bool(false)),
            (
                "min",
                Value::record([("x", Value::from(0.0)), ("y", Value::from(0.0))]),
            ),
            (
                "max",
                Value::record([("x", Value::from(404.0)), ("y", Value::from(32.0))]),
            ),
        ]),
        "is_item_hovered" | "is_item_active" | "is_mouse_dragging" | "button" => {
            Value::record([("ok", Value::Bool(true)), ("value", Value::Bool(false))])
        }
        "mouse_delta" => Value::record([
            ("ok", Value::Bool(true)),
            ("x", Value::from(0.0)),
            ("y", Value::from(0.0)),
        ]),
        _ => Value::record([("ok", Value::Bool(true))]),
    }
}

fn push_frame(app: &TestApp, seq: f64) {
    app.host
        .shared(&format!("plugin.{ID}.calls"), Value::Nil)
        .set(vlist(vec![Value::record([
            ("id", Value::from(format!("frame-{seq}").as_str())),
            ("kind", Value::from("imgui.frame")),
            ("window", Value::from(WINDOW)),
            ("seq", Value::from(seq)),
            ("budget", Value::from(8.0)),
        ])]));
}

fn ops(heard: &Heard, op: &str) -> Vec<Json> {
    heard
        .borrow()
        .iter()
        .filter_map(|line| serde_json::from_str::<Json>(line).ok())
        .filter(|asked| asked["op"].as_str() == Some(op))
        .collect()
}

#[test]
fn the_custom_chrome_fixture_opens_its_window_and_draws_its_bar() {
    let m = manifest();
    assert_eq!(m["id"], Json::from(ID), "{m}");
    assert_eq!(m["entry"], Json::from("init.luau"), "{m}");
    assert_eq!(
        m["permissions"],
        Json::from(vec![Json::from("imgui.windows")]),
        "{m}"
    );

    let (app, heard) = harness();
    app.frames(30);
    assert_eq!(
        app.error(),
        None,
        "the fixture did not load: {:?}",
        app.error()
    );

    let opened = ops(&heard, "open_window");
    assert_eq!(opened.len(), 1, "one window: {opened:?}");
    assert_eq!(opened[0]["id"], Json::from(WINDOW), "{}", opened[0]);
    assert_eq!(opened[0]["title"], Json::from(TITLE), "{}", opened[0]);
    assert_eq!(opened[0]["titlebar"], Json::from("hidden"), "{}", opened[0]);
    assert!(
        opened[0]["decorations"].as_bool().is_none_or(|d| !d),
        "{}",
        opened[0]
    );

    let rendered = ops(&heard, "render");
    assert_eq!(rendered.len(), 1, "{rendered:?}");
    assert_eq!(rendered[0]["window"], Json::from(WINDOW), "{rendered:?}");

    push_frame(&app, 1.0);
    app.frames(3);
    let at = ops(&heard, "is_maximized");
    assert_eq!(at.len(), 1, "the bar reads the maximized state: {at:?}");
    let bars = ops(&heard, "invisible_button");
    assert_eq!(bars.len(), 1, "one drag bar: {bars:?}");
    assert_eq!(bars[0]["id"], Json::from("drag"), "{}", bars[0]);
    assert_eq!(
        bars[0]["size"]["width"].as_f64(),
        Some(404.0),
        "{}",
        bars[0]
    );
    assert_eq!(
        bars[0]["size"]["height"].as_f64(),
        Some(32.0),
        "{}",
        bars[0]
    );
    let labels: Vec<String> = ops(&heard, "button")
        .into_iter()
        .filter_map(|asked| asked["label"].as_str().map(str::to_string))
        .collect();
    for label in ["minimize", "maximize", "close", "resize"] {
        assert!(
            labels.iter().any(|said| said == label),
            "{label}: {labels:?}"
        );
    }
}
