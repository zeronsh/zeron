//! The `zeron.imgui` SDK, end to end against a host that plays the host side.
//!
//! A real Luau plugin VM loads the SDK from `plugins-sdk/zeron.luau` (copied
//! into a temporary plugin folder, as the installer does) and the test plays
//! the host: `plugin.p.imgui` answers the window and widget ops, and
//! `plugin.p.imgui_end` records how each frame closed. The frame requests go
//! through `plugin.p.calls`, exactly the channel the production pump uses.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::Value as Json;
use zgx_script::testing::TestApp;
use zgx_script::{Plugin, ScriptHost, Value};

use crate::plugins::Item;
use crate::plugins::imgui::permission;
use crate::plugins::vlist;

/// What the fake host heard, as JSON strings, in order.
type Heard = Rc<RefCell<Vec<String>>>;

const PLUGIN: &str = r#"
local zeron = require("./zeron")("p")

local said = ui.state("said", "")
local function say(words: string)
	local old = said:peek()
	said:set(if old == "" then words else old .. "|" .. words)
end

ui.contribute("bar", ui.text { id = "said", function() return said:get() end })

local w = zeron.imgui.open_window { id = "w", title = "Widgets", size = { width = 400, height = 300 } }
local kept: any = nil
w:on_render(function(view)
	kept = view
	view.text("hello")
	if view.button("B") then
		say("clicked")
	end
end)
w:on_close(function(closed: string)
	say("closed:" .. closed)
end)

-- A second window re-using the first window's `ui`, the way a plugin that
-- keeps one might: the host refuses it and the error must reach the host.
local v = zeron.imgui.open_window { id = "v", title = "Second" }
v:on_render(function(_view)
	kept.text("FAILME")
end)
"#;

/// The host side: answers the ops, and records each frame's end.
fn harness(name: &str, heard: Heard) -> TestApp {
    let dir = std::env::temp_dir().join(format!("zgx-imgui-sdk-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sdk = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins-sdk")
            .join("zeron.luau"),
    )
    .expect("the SDK file");
    std::fs::write(dir.join("zeron.luau"), sdk).unwrap();
    let entry = dir.join("init.luau");
    std::fs::write(&entry, PLUGIN).unwrap();

    let heard_in = heard.clone();
    TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        let heard = heard_in;
        // What the app exposes to a plugin (`expose_surface` and
        // `expose_own` in production): the keys it may read, its own keys,
        // and the two actions the SDK calls.
        for (key, _) in crate::plugins::grants::SHARED_READ {
            host.expose_shared_read(*key, None);
            host.shared(key, Value::Nil);
        }
        for name in crate::plugins::grants::OWN_SHARED {
            host.expose_shared_read(format!("plugin.p.{name}"), None);
            // Lists where the engine sends lists, records elsewhere: the
            // SDK reads each as the shape its listing gives it.
            let empty = match *name {
                "calls" | "inbox" | "menu_items" | "locales" | "composed" => vlist(Vec::new()),
                _ => Value::record(std::iter::empty::<(&str, Value)>()),
            };
            host.shared(&format!("plugin.p.{name}"), Value::Nil)
                .set(empty);
        }
        host.expose_action("plugin.p.imgui", None);
        host.expose_action("plugin.p.imgui_end", None);
        for name in crate::plugins::grants::OWN_ACTIONS {
            host.expose_action(format!("plugin.p.{name}"), None);
        }
        let log = heard.clone();
        host.function("plugin.p.imgui", move |arg| {
            let asked: Json = serde_json::from_str(&arg.to_json()).unwrap_or_default();
            host_answer(&log, &asked)
        });
        let ended = heard.clone();
        host.action("plugin.p.imgui_end", move |arg| {
            ended.borrow_mut().push(format!("end {}", arg.to_json()));
        });
        let answered = heard.clone();
        host.action("plugin.p.result", move |arg| {
            answered
                .borrow_mut()
                .push(format!("result {}", arg.to_json()));
        });
        host.set_plugins(vec![Plugin {
            name: "p".into(),
            entry,
            grants: vec![],
        }]);
        host
    })
}

/// What the host answers for one op, and what it records.
fn host_answer(log: &Heard, asked: &Json) -> Value {
    let op = asked["op"].as_str().unwrap_or_default();
    log.borrow_mut().push(asked.to_string());
    match op {
        "open_window" => Value::record([
            ("ok", Value::Bool(true)),
            ("id", Value::from(asked["id"].as_str().unwrap_or_default())),
        ]),
        "text" if asked["text"].as_str() == Some("FAILME") => Value::record([
            ("ok", Value::Bool(false)),
            (
                "error",
                Value::from(
                    "ui.text was called outside window \"w\"'s render callback. Draw with the ui passed to that window's on_render, inside the callback.",
                ),
            ),
        ]),
        "text" => Value::record([
            ("ok", Value::Bool(true)),
            (
                "min",
                Value::record([("x", Value::from(1.0)), ("y", Value::from(2.0))]),
            ),
            (
                "max",
                Value::record([("x", Value::from(30.0)), ("y", Value::from(20.0))]),
            ),
        ]),
        "button" => Value::record([("ok", Value::Bool(true)), ("value", Value::Bool(true))]),
        "is_open" => Value::record([("ok", Value::Bool(true)), ("open", Value::Bool(true))]),
        _ => Value::record([("ok", Value::Bool(true))]),
    }
}

/// Pushes one call into the plugin's queue, the way the pump does.
fn push(app: &TestApp, call: Value) {
    app.host
        .shared("plugin.p.calls", Value::Nil)
        .set(vlist(vec![call]));
}

fn frame_call(id: &str, window: &str, seq: f64) -> Value {
    Value::record([
        ("id", Value::from(id)),
        ("kind", Value::from("imgui.frame")),
        ("window", Value::from(window)),
        ("seq", Value::from(seq)),
        ("budget", Value::from(8.0)),
    ])
}

#[test]
fn the_sdk_opens_a_window_and_draws_in_it() {
    let heard: Heard = Heard::default();
    let mut app = harness("DRAWN", heard.clone());
    app.frames(30);
    assert_eq!(
        app.error(),
        None,
        "the plugin did not load: {:?}",
        app.error()
    );

    let opened: Vec<String> = heard
        .borrow()
        .iter()
        .filter(|line| line.contains("\"op\":\"open_window\""))
        .cloned()
        .collect();
    assert_eq!(opened.len(), 2, "two windows: {opened:?}");
    assert!(opened[0].contains("\"id\":\"w\""), "{}", opened[0]);
    assert!(opened[1].contains("\"id\":\"v\""), "{}", opened[1]);
    assert!(
        heard
            .borrow()
            .iter()
            .any(|line| line.contains("\"op\":\"render\"")),
        "on_render was registered with the host: {:?}",
        heard.borrow()
    );

    // A frame for `w`: the SDK draws the text and reads the button's answer.
    heard.borrow_mut().clear();
    push(&app, frame_call("c1", "w", 1.0));
    app.frames(3);
    let log = heard.borrow().clone();
    assert!(
        log.iter()
            .any(|line| line.contains("\"op\":\"text\"") && line.contains("hello")),
        "{log:?}"
    );
    let button_at = log
        .iter()
        .position(|line| line.contains("\"op\":\"button\""))
        .unwrap_or_else(|| panic!("no button call: {log:?}"));
    assert!(
        log[button_at].contains("\"label\":\"B\""),
        "{}",
        log[button_at]
    );
    let end = log
        .iter()
        .find(|line| line.starts_with("end "))
        .unwrap_or_else(|| panic!("the frame was not ended: {log:?}"));
    assert!(end.contains("\"window\":\"w\""), "{end}");
    assert!(end.contains("\"seq\":1"), "{end}");
    assert!(end.contains("\"ok\":true"), "{end}");
    assert_eq!(
        app.text("said"),
        "clicked",
        "the button's true answer reached the plugin"
    );
}

#[test]
fn the_sdk_reports_a_widget_error_to_the_host_and_keeps_loading() {
    let heard: Heard = Heard::default();
    let app = harness("FAILED", heard.clone());
    app.frames(30);
    assert_eq!(app.error(), None, "the plugin did not load");

    heard.borrow_mut().clear();
    // `w` draws once, so the plugin holds its `ui` …
    push(&app, frame_call("c1", "w", 1.0));
    app.frames(3);
    heard.borrow_mut().clear();
    // … and `v`'s callback draws with it.
    push(&app, frame_call("c2", "v", 1.0));
    app.frames(3);
    let log = heard.borrow().clone();
    let end = log
        .iter()
        .find(|line| line.starts_with("end "))
        .unwrap_or_else(|| panic!("the frame was not ended: {log:?}"));
    assert!(end.contains("\"window\":\"v\""), "{end}");
    assert!(
        end.contains("\"ok\":false"),
        "the host heard the callback failed: {end}"
    );
    assert!(
        end.contains("outside window \\\"w\\\"'s render callback"),
        "the reason names the window and what was wrong: {end}"
    );
    assert!(
        end.contains("\"refused\":true"),
        "the host hears this was a refusal, not the plugin's own error: {end}"
    );
    assert_eq!(
        app.error(),
        None,
        "a widget error is not a script crash: the plugin stays loaded"
    );

    // The plugin still runs: another frame for `w` draws.
    push(&app, frame_call("c3", "w", 2.0));
    app.frames(3);
    assert!(
        heard
            .borrow()
            .iter()
            .any(|line| line.contains("\"op\":\"text\"")),
        "the plugin kept drawing"
    );
}

#[test]
fn the_sdk_hands_a_close_to_the_window_s_on_close_once() {
    let heard: Heard = Heard::default();
    let mut app = harness("CLOSE", heard.clone());
    app.frames(30);
    assert_eq!(app.error(), None, "the plugin did not load");

    push(
        &app,
        Value::record([
            ("id", Value::from("close-1")),
            ("kind", Value::from("imgui.close")),
            ("window", Value::from("w")),
        ]),
    );
    app.frames(5);
    assert_eq!(
        app.text("said"),
        "closed:w",
        "the window's on_close ran once, with its id"
    );
    assert!(
        heard
            .borrow()
            .iter()
            .any(|line| line.starts_with("result ")),
        "the close call was answered: {:?}",
        heard.borrow()
    );

    // A second close call for the same id reaches the plugin as a second
    // call only if the host queued one; the SDK does not invent one, and a
    // closed handle's methods refuse rather than run the callback again.
    push(
        &app,
        Value::record([
            ("id", Value::from("close-2")),
            ("kind", Value::from("imgui.close")),
            ("window", Value::from("w")),
        ]),
    );
    app.frames(3);
    assert!(
        app.text("said").ends_with("closed:w"),
        "the second call is not a second close: {}",
        app.text("said")
    );
}

// -- A plugin the `imgui.windows` gate turned away -------------------------
//
// The VM refuses `plugin.<id>.imgui` before the host action runs, so the
// plugin would otherwise be handed zgx's capability name
// (`Named("zeron.imgui.windows")`). The SDK asks
// `plugin.<id>.imgui_refused` for the host's sentence — recorded once for
// the plugin's diagnostics — and raises that in its place. These tests
// expose both actions the way `imgui::luau::serve` does.

/// The sentence the host uses for a plugin that may not open windows.
const SENTENCE: &str = "This plugin wasn't allowed to open native application windows. Allow \"imgui.windows\" for it in Zeron.";

/// A plugin that asks for a window twice, keeping what each attempt said.
const DENIED: &str = r#"
local zeron = require("./zeron")("p")

local said = ui.state("said", "")
local function say(words: string)
	local old = said:peek()
	said:set(if old == "" then words else old .. "|" .. words)
end
ui.contribute("bar", ui.text { id = "said", function() return said:get() end })

local function attempt()
	local ok, why = pcall(function()
		zeron.imgui.open_window { id = "x", title = "Denied" }
	end)
	say(tostring(ok) .. "|" .. tostring(why))
end
attempt()
attempt()
"#;

/// What the fake host heard of a refused plugin: the sentences it was asked
/// for, and whether the gated ImGui action was reached at all.
#[derive(Clone, Default)]
struct Refused {
    recorded: Rc<RefCell<Vec<String>>>,
    reached: Rc<Cell<usize>>,
}

/// A plugin VM holding `granted`, listed as `listing`, with both actions
/// exposed as production exposes them.
fn refused_harness(name: &str, listing: Item, granted: &[&str]) -> (TestApp, Refused) {
    let dir = std::env::temp_dir().join(format!("zgx-imgui-refused-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sdk = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("plugins-sdk")
            .join("zeron.luau"),
    )
    .expect("the SDK file");
    std::fs::write(dir.join("zeron.luau"), sdk).unwrap();
    let entry = dir.join("init.luau");
    std::fs::write(&entry, DENIED).unwrap();

    let heard = Refused::default();
    let asked: Vec<String> = granted
        .iter()
        .map(|permission| permission.to_string())
        .collect();
    let grants = crate::plugins::grants::capabilities("p", &asked, &[], &[]);
    let host_side = heard.clone();
    let host_listing = listing.clone();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        for (key, _) in crate::plugins::grants::SHARED_READ {
            host.expose_shared_read(*key, None);
            host.shared(key, Value::Nil);
        }
        for name in crate::plugins::grants::OWN_SHARED {
            host.expose_shared_read(format!("plugin.p.{name}"), None);
            let empty = match *name {
                "calls" | "inbox" | "menu_items" | "locales" | "composed" => vlist(Vec::new()),
                _ => Value::record(std::iter::empty::<(&str, Value)>()),
            };
            host.shared(&format!("plugin.p.{name}"), Value::Nil)
                .set(empty);
        }
        for name in crate::plugins::grants::OWN_ACTIONS {
            host.expose_action(format!("plugin.p.{name}"), None);
        }
        // The gated action: a VM without the capability never reaches it.
        permission::expose_action(&host, "plugin.p.imgui");
        {
            let (reached, listing) = (host_side.reached.clone(), host_listing.clone());
            host.function("plugin.p.imgui", move |_| {
                reached.set(reached.get() + 1);
                match permission::open_window(Some(&listing)) {
                    Ok(()) => Value::record([("ok", Value::Bool(true))]),
                    Err(sentence) => Value::record([
                        ("ok", Value::Bool(false)),
                        ("error", Value::from(sentence.as_str())),
                    ]),
                }
            });
        }
        // The reporter, exposed without the capability: the host's own
        // sentence, recorded once for the plugin's diagnostics.
        host.expose_action("plugin.p.imgui_refused", None);
        {
            let (recorded, listing) = (host_side.recorded.clone(), host_listing.clone());
            host.function("plugin.p.imgui_refused", move |_| {
                let sentence = permission::open_window_refusal(Some(&listing));
                recorded.borrow_mut().push(sentence.clone());
                Value::record([
                    ("ok", Value::Bool(false)),
                    ("error", Value::from(sentence.as_str())),
                ])
            });
        }
        host.set_plugins(vec![Plugin {
            name: "p".into(),
            entry,
            grants,
        }]);
        host
    });
    (app, heard)
}

fn plugin_listing(permissions: &[&str], allowed: Option<&[&str]>) -> Item {
    let mut json = serde_json::json!({ "id": "p", "status": "on", "permissions": permissions });
    if let Some(allowed) = allowed {
        json["allowed"] = serde_json::json!(allowed);
    }
    Item { json }
}

#[test]
fn a_vm_without_the_grant_raises_the_hosts_sentence() {
    let (mut app, refused) = refused_harness("denied", plugin_listing(&[], None), &[]);
    app.frames(20);
    assert_eq!(
        app.error(),
        None,
        "a refused permission is not a script error: {:?}",
        app.error()
    );
    let said = app.text("said");
    assert_eq!(
        said.matches(SENTENCE).count(),
        2,
        "both attempts raise the host's sentence: {said}"
    );
    assert_eq!(
        said.matches("false|").count(),
        2,
        "open_window failed both times: {said}"
    );
    assert!(
        !said.contains("Named("),
        "the VM's capability words never reach the plugin: {said}"
    );
    assert_eq!(
        refused.reached.get(),
        0,
        "a VM without the capability never reaches the host action"
    );
    assert_eq!(
        refused.recorded.borrow().len(),
        1,
        "one diagnostics error for two attempts: {:?}",
        refused.recorded.borrow()
    );
}

#[test]
fn the_per_call_gate_s_sentence_reaches_the_plugin_as_itself() {
    // The VM holds the capability, but the engine no longer allows the
    // permission: the call reaches the host and its per-call gate refuses.
    let listing = plugin_listing(&["imgui.windows"], Some(&[]));
    let (mut app, refused) = refused_harness("revoked", listing, &["imgui.windows"]);
    app.frames(20);
    assert_eq!(
        app.error(),
        None,
        "the plugin stays loaded: {:?}",
        app.error()
    );
    let said = app.text("said");
    assert_eq!(
        said.matches(SENTENCE).count(),
        2,
        "the host's per-call sentence is the error: {said}"
    );
    assert_eq!(refused.reached.get(), 2, "every attempt reached the gate");
    assert!(
        refused.recorded.borrow().is_empty(),
        "the host answered with the sentence itself, so nothing had to be asked for"
    );
}

/// `Bridge::widget_refused` and a dev plugin's log both say each distinct
/// sentence once: a fixture retrying a refused call reads one error.
#[test]
fn a_retried_refusal_is_reported_once() {
    let heard: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = heard.clone();
    let report = crate::plugins::imgui::luau::once(move |sentence: &str| {
        sink.borrow_mut().push(sentence.to_string());
    });
    report(SENTENCE);
    report(SENTENCE);
    report("another sentence");
    assert_eq!(
        *heard.borrow(),
        vec![SENTENCE.to_string(), "another sentence".to_string()]
    );
}
