//! The shipped `plugins-dev/imgui-hello` fixture, headless: the folder the
//! validators load in the real app, loaded from its own directory exactly as
//! the bridge loads a snapshot (its `init.luau` beside its own copy of the
//! SDK), with this test playing the host side of `plugin.<id>.imgui`.
//!
//! What it proves: the fixture loads with no script error, opens exactly one
//! window with the documented id, title and size, draws its text and its
//! button on each frame the host asks for, and runs the button's branch once
//! per release — the line the click prints is on that branch, so one click is
//! one print.
//!
//! The real window, its pixels and the print in the app's log are the native
//! check (`ZERON_ZGX_DEV_PLUGINS` + `ZERON_ZGX_PLUGIN_LOG=1`); this test is
//! the protocol half, with no desktop.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::Value as Json;
use zgx_script::testing::TestApp;
use zgx_script::{Plugin, ScriptHost, Value};

use crate::plugins::vlist;

const ID: &str = "imgui-hello";
const WINDOW: &str = "hello";
/// What the fixture says the window is, so a native probe knows what to find.
const TITLE: &str = "Hello from Luau";
const WIDTH: f64 = 520.0;
const HEIGHT: f64 = 300.0;

/// The plugin's own folder, as the app's dev switch names it.
fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("plugins-dev")
        .join(ID)
}

/// Every op the fake host was asked, as JSON strings, and each frame's end.
type Heard = Rc<RefCell<Vec<String>>>;

/// The host side: answers the window and widget ops, and says `true` for the
/// button on the frames the test sets [`Pressed`] for, as a release does.
#[derive(Clone, Default)]
struct Pressed(Rc<Cell<bool>>);

fn harness() -> (TestApp, Heard, Pressed) {
    let entry = folder().join("init.luau");
    let heard: Heard = Heard::default();
    let pressed = Pressed::default();

    let log = heard.clone();
    let button = pressed.clone();
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
            host_answer(&answered, &button, &arg.to_json())
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
    (app, heard, pressed)
}

/// What the host answers for one op, the way `imgui::luau` does (as JSON, the
/// fake reads it back from the call it recorded).
fn host_answer(log: &Heard, pressed: &Pressed, asked: &str) -> Value {
    let asked: Json = serde_json::from_str(asked).unwrap_or_default();
    log.borrow_mut().push(asked.to_string());
    match asked["op"].as_str().unwrap_or_default() {
        "open_window" => Value::record([
            ("ok", Value::Bool(true)),
            ("id", Value::from(asked["id"].as_str().unwrap_or_default())),
        ]),
        // The release frame: the plugin's branch runs then and only then.
        "button" => Value::record([
            ("ok", Value::Bool(true)),
            ("value", Value::Bool(pressed.0.get())),
        ]),
        "text" => Value::record([
            ("ok", Value::Bool(true)),
            (
                "min",
                Value::record([("x", Value::from(0.0)), ("y", Value::from(0.0))]),
            ),
            (
                "max",
                Value::record([("x", Value::from(200.0)), ("y", Value::from(18.0))]),
            ),
        ]),
        _ => Value::record([("ok", Value::Bool(true))]),
    }
}

/// Pushes one frame request into the plugin's queue, as the pump does.
fn push(app: &TestApp, seq: f64) {
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

/// The ops of one kind the host heard, parsed.
fn ops(log: &Heard, op: &str) -> Vec<Json> {
    log.borrow()
        .iter()
        .filter_map(|line| serde_json::from_str::<Json>(line).ok())
        .filter(|asked| asked["op"].as_str() == Some(op))
        .collect()
}

/// Every `text` the plugin drew, in order.
fn drawn(log: &Heard) -> Vec<String> {
    ops(log, "text")
        .iter()
        .filter_map(|asked| asked["text"].as_str().map(str::to_string))
        .collect()
}

fn number(value: &Json) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

/// The fixture's manifest: the id, the entry and the permission it asks for
/// (and nothing it does not use).
fn manifest() -> Json {
    let text = std::fs::read_to_string(folder().join("zeron-plugin.json")).expect("its manifest");
    serde_json::from_str(&text).expect("a manifest")
}

#[test]
fn the_hello_fixture_loads_and_opens_its_one_window() {
    let manifest = manifest();
    assert_eq!(manifest["id"], Json::from(ID), "{manifest}");
    assert_eq!(manifest["entry"], Json::from("init.luau"), "{manifest}");
    assert_eq!(
        manifest["permissions"],
        Json::from(vec![Json::from("imgui.windows")]),
        "it asks for imgui.windows and nothing it does not use: {manifest}"
    );

    let (app, heard, pressed) = harness();
    app.frames(30);
    assert_eq!(
        app.error(),
        None,
        "the fixture did not load: {:?}",
        app.error()
    );

    // One window, with the id, title and size the folder's README names.
    let opened = ops(&heard, "open_window");
    assert_eq!(opened.len(), 1, "exactly one window: {opened:?}");
    assert_eq!(opened[0]["id"], Json::from(WINDOW), "{}", opened[0]);
    assert_eq!(opened[0]["title"], Json::from(TITLE), "{}", opened[0]);
    assert_eq!(number(&opened[0]["size"]["width"]), WIDTH, "{}", opened[0]);
    assert_eq!(
        number(&opened[0]["size"]["height"]),
        HEIGHT,
        "{}",
        opened[0]
    );

    // Its render callback is registered once, for that window.
    let registered = ops(&heard, "render");
    assert_eq!(registered.len(), 1, "{registered:?}");
    assert_eq!(
        registered[0]["window"],
        Json::from(WINDOW),
        "{registered:?}"
    );
    assert_eq!(registered[0]["on"], Json::Bool(true), "{registered:?}");

    // A frame draws the documented text, the button, and the click count at 0.
    heard.borrow_mut().clear();
    push(&app, 1.0);
    app.frames(3);
    let first = drawn(&heard);
    assert!(
        first
            .iter()
            .any(|line| line == "Hello from Luau! This window is drawn by a plugin."),
        "{first:?}"
    );
    assert!(first.iter().any(|line| line == "frame 1"), "{first:?}");
    assert!(first.iter().any(|line| line == "clicks 0"), "{first:?}");
    let buttons = ops(&heard, "button");
    assert_eq!(buttons.len(), 1, "one button a frame: {buttons:?}");
    assert_eq!(buttons[0]["label"], Json::from("Click me"), "{buttons:?}");
    assert!(ended_frame(&heard).is_some(), "the frame was ended");

    // Another frame counts once more: the callback runs per request.
    heard.borrow_mut().clear();
    push(&app, 2.0);
    app.frames(3);
    assert!(
        drawn(&heard).iter().any(|line| line == "frame 2"),
        "{:?}",
        drawn(&heard)
    );

    // A release answers true once: the button's branch runs once, so exactly
    // one line is printed (the clicks line is the same branch's state).
    pressed.0.set(true);
    heard.borrow_mut().clear();
    push(&app, 3.0);
    app.frames(3);
    pressed.0.set(false);
    let clicked = drawn(&heard);
    assert!(
        clicked.iter().any(|line| line == "clicks 1"),
        "the click ran once: {clicked:?}"
    );

    // Pressing never runs it; later frames keep the count at one.
    for seq in [4.0, 5.0] {
        heard.borrow_mut().clear();
        push(&app, seq);
        app.frames(3);
        let after = drawn(&heard);
        assert!(
            after.iter().any(|line| line == "clicks 1"),
            "frame {seq} kept the count: {after:?}"
        );
        assert!(
            after.iter().all(|line| line != "clicks 2"),
            "no second click: {after:?}"
        );
    }

    // Nothing the plugin did was a script error, and no frame was refused.
    assert_eq!(app.error(), None, "{:?}", app.error());
    let ends: Vec<String> = heard
        .borrow()
        .iter()
        .filter(|line| line.starts_with("end "))
        .cloned()
        .collect();
    assert!(
        ends.iter().all(|end| end.contains("\"ok\":true")),
        "every frame finished cleanly: {ends:?}"
    );
}

/// The frame the plugin runs says it ended for its own window and sequence.
#[test]
fn each_frame_ends_with_no_error() {
    let (app, heard, _) = harness();
    app.frames(30);
    assert_eq!(app.error(), None);
    for seq in 1..=3 {
        heard.borrow_mut().clear();
        push(&app, f64::from(seq));
        app.frames(3);
        let end = heard
            .borrow()
            .iter()
            .find(|line| line.starts_with("end "))
            .cloned()
            .unwrap_or_else(|| panic!("frame {seq} was not ended: {:?}", heard.borrow()));
        assert!(end.contains(&format!("\"window\":\"{WINDOW}\"")), "{end}");
        assert!(end.contains(&format!("\"seq\":{seq}")), "{end}");
        assert!(end.contains("\"ok\":true"), "{end}");
    }
}

/// The SDK in the folder is the shipped one: a plugin carries a copy, and a
/// stale copy is a plugin that behaves like an older SDK.
#[test]
fn the_hello_fixture_carries_the_shipped_sdk() {
    let shipped = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins-sdk")
            .join("zeron.luau"),
    )
    .expect("plugins-sdk/zeron.luau");
    let copied = std::fs::read_to_string(folder().join("zeron.luau")).expect("its zeron.luau");
    assert_eq!(
        copied, shipped,
        "plugins-dev/{ID}/zeron.luau is not the shipped SDK: copy plugins-sdk/zeron.luau over it"
    );
}

/// The frame's end, parsed (the line the fake host recorded).
fn ended_frame(log: &Heard) -> Option<Json> {
    log.borrow()
        .iter()
        .find(|line| line.starts_with("end "))
        .and_then(|line| serde_json::from_str::<Json>(line.trim_start_matches("end ")).ok())
}
