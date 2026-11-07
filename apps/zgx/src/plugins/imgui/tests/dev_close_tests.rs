//! A dev plugin's closed windows, told through its own call queue.
//!
//! `ZERON_ZGX_DEV_PLUGINS` gives a plugin no bridge, so the installed
//! path's `{ kind = "imgui.close", window }` is queued over the same
//! `plugin.<id>.calls` state the frame pump uses (`luau::dev_listen`,
//! whose queue and close handler `dev_plugins` hands to `serve`). These
//! tests drive a real VM with that channel: the queue carries the call,
//! and the plugin's `on_close` and its closed-handle guards behave as they
//! do for an installed plugin (VAL-WIN-013, VAL-WIN-014).

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::Value as Json;
use zgx_script::testing::TestApp;
use zgx_script::{Plugin, ScriptHost, Value};

use crate::plugins::imgui::frame::FrameCalls;
use crate::plugins::imgui::luau;
use crate::plugins::vlist;

/// What the fake host heard, in order.
type Heard = Rc<RefCell<Vec<String>>>;

/// A plugin that watches its own call queue: it records every non-empty
/// snapshot, so the test sees what the host actually queued.
const WATCHER: &str = r#"
local calls = ui.shared("plugin.p.calls", {})
local seen = ui.state("seen", "")
ui.contribute("bar", ui.text { id = "seen", function() return seen:get() end })
ui.effect(function()
	local parts = {}
	for _, call in calls:get() do
		table.insert(parts, `{call.kind}:{tostring(call.window)}:{tostring(call.id)}`)
	end
	if #parts > 0 then
		local old = seen:peek()
		seen:set(if old == "" then table.concat(parts, ",") else old .. "|" .. table.concat(parts, ","))
	end
end)
"#;

/// A plugin with two windows through the SDK: `w` reports its own close and
/// every retained-handle call after it, and `s` keeps drawing.
const TWO_WINDOWS: &str = r#"
local zeron = require("./zeron")("p")

local said = ui.state("said", "")
local function say(words: string)
	local old = said:peek()
	said:set(if old == "" then words else old .. "|" .. words)
end
ui.contribute("bar", ui.text { id = "said", function() return said:get() end })

local function probe(label: string, run: () -> ())
	local ok, why = pcall(run)
	say(`{label}:{tostring(ok)}:{tostring(why)}`)
end

local w = zeron.imgui.open_window { id = "w", title = "Probe W", size = { width = 400, height = 300 } }
w:on_render(function(view)
	say("drew:w")
	view.text("w")
end)
w:on_close(function(closed: string)
	say("closed:" .. closed)
	say("is_open:" .. tostring(w:is_open()))
	probe("close_again", function() w:close() end)
	probe("show", function() w:show() end)
	probe("set_title", function() w:set_title("again") end)
	probe("on_render", function() w:on_render(function() end) end)
end)

local s = zeron.imgui.open_window { id = "s", title = "Sibling" }
s:on_render(function(view)
	say("drew:s")
	view.text("s")
end)
"#;

/// A TestApp with `script` as plugin `p`, and no engine.
fn harness(name: &str, script: &str, heard: Heard) -> TestApp {
    let dir =
        std::env::temp_dir().join(format!("zgx-imgui-dev-close-{}-{name}", std::process::id()));
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
    std::fs::write(&entry, script).unwrap();

    TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        // What the app exposes to a plugin, as `dev_plugins` does.
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
        host.expose_action("plugin.p.imgui", None);
        host.expose_action("plugin.p.imgui_end", None);
        for name in crate::plugins::grants::OWN_ACTIONS {
            host.expose_action(format!("plugin.p.{name}"), None);
        }
        let log = heard.clone();
        host.function("plugin.p.imgui", move |arg| {
            let asked: Json = serde_json::from_str(&arg.to_json()).unwrap_or_default();
            log.borrow_mut().push(asked.to_string());
            if asked["op"].as_str() == Some("open_window") {
                return Value::record([
                    ("ok", Value::Bool(true)),
                    ("id", Value::from(asked["id"].as_str().unwrap_or_default())),
                ]);
            }
            Value::record([
                ("ok", Value::Bool(true)),
                (
                    "min",
                    Value::record([("x", Value::from(1.0)), ("y", Value::from(2.0))]),
                ),
                (
                    "max",
                    Value::record([("x", Value::from(30.0)), ("y", Value::from(20.0))]),
                ),
            ])
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

/// One frame request as the pump queues it, taken back once the plugin has
/// answered (which `frame::run` does after every frame).
fn frame(calls: &FrameCalls, window: &str, seq: f64) {
    let call = format!("imgui-frame-{window}-{seq}");
    calls.push(
        call.clone(),
        "imgui.frame",
        vec![
            ("window", Value::from(window)),
            ("seq", Value::from(seq)),
            ("budget", Value::from(8.0)),
        ],
    );
    calls.take(&call);
}

/// How many times `needle` occurs in `haystack`.
fn times(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

#[test]
fn a_dev_plugin_is_told_of_a_close_on_the_queue_that_carries_its_frames() {
    let heard: Heard = Heard::default();
    let mut app = harness("QUEUE", WATCHER, heard);
    app.frames(10);
    assert_eq!(
        app.error(),
        None,
        "the plugin did not load: {:?}",
        app.error()
    );

    let (calls, closed) = luau::dev_listen(&app.host, "p", |_| {});
    frame(&calls, "w", 1.0);
    closed("w");

    // The frame and the close arrive through the one queue: the plugin saw
    // both, in order, and the close carries the window it was for.
    assert_eq!(
        app.text("seen"),
        "imgui.frame:w:imgui-frame-w-1|imgui.close:w:imgui-close-w",
        "the close did not come through the frame channel"
    );

    // A window id reused after a reopen is a fresh close: the answered call
    // was taken off the queue, so the SDK runs it again rather than believe
    // it already did.
    closed("w");
    let seen = app.text("seen");
    assert_eq!(
        times(&seen, "imgui.close:w:imgui-close-w"),
        2,
        "a second close of the same window id was not a fresh call: {seen}"
    );

    // Nothing is left behind: the queue is empty once the plugin answered.
    let left = app
        .host
        .shared("plugin.p.calls", Value::Nil)
        .signal()
        .with_untracked(|value| value.list().len());
    assert_eq!(left, 0, "the answered calls were not taken off the queue");
}

#[test]
fn a_dev_plugin_runs_on_close_once_and_its_retained_handle_is_inert() {
    let heard: Heard = Heard::default();
    let mut app = harness("CLOSE", TWO_WINDOWS, heard.clone());
    app.frames(20);
    assert_eq!(
        app.error(),
        None,
        "the plugin did not load: {:?}",
        app.error()
    );

    let (calls, closed) = luau::dev_listen(&app.host, "p", |_| {});
    // Both windows draw through the queue before anything closes.
    frame(&calls, "w", 1.0);
    frame(&calls, "s", 1.0);
    let said = app.text("said");
    assert_eq!(times(&said, "drew:w"), 1, "w did not draw: {said}");
    assert_eq!(times(&said, "drew:s"), 1, "s did not draw: {said}");

    // The same call an installed plugin gets, on the dev channel.
    closed("w");
    let said = app.text("said");
    assert_eq!(
        times(&said, "closed:w"),
        1,
        "the dev plugin's on_close did not run exactly once: {said}"
    );
    assert_eq!(
        times(&said, "closed:s"),
        0,
        "the sibling's on_close ran for another window: {said}"
    );
    assert!(
        said.contains("is_open:false"),
        "the retained handle still reports open: {said}"
    );
    assert!(
        said.contains("close_again:true"),
        "closing the retained handle again raised: {said}"
    );
    for label in ["show", "set_title", "on_render"] {
        let line = said
            .split('|')
            .find(|line| line.starts_with(&format!("{label}:")))
            .unwrap_or_else(|| panic!("{label} was not probed: {said}"));
        assert!(
            line.starts_with(&format!("{label}:false:")),
            "{label} did not refuse the closed handle: {line}"
        );
        assert!(
            line.contains("Window \"w\" is closed"),
            "{label}'s error does not name the closed window: {line}"
        );
    }

    // The sibling keeps rendering, and the closed window's callback does not
    // run again for a frame the pump might still ask for.
    frame(&calls, "s", 2.0);
    frame(&calls, "w", 2.0);
    let said = app.text("said");
    assert_eq!(
        times(&said, "drew:s"),
        2,
        "the sibling stopped drawing after its sibling closed: {said}"
    );
    assert_eq!(
        times(&said, "drew:w"),
        1,
        "the closed window's on_render ran again: {said}"
    );

    // A second delivery of the same close (which the host does not make) is
    // not a second on_close: the SDK dropped the handle with the window.
    closed("w");
    assert_eq!(
        times(&app.text("said"), "closed:w"),
        1,
        "the same window was closed twice: {}",
        app.text("said")
    );

    // The plugin answered the call the way an installed one does.
    assert!(
        heard
            .borrow()
            .iter()
            .any(|line| line.starts_with("result ") && line.contains("imgui-close-w")),
        "the close call was not answered: {:?}",
        heard.borrow()
    );
}
