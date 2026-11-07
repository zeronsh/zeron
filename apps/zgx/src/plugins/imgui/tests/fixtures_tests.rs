//! The validation contract's fixture plugins (`plugins-dev/`), headless.
//!
//! Each folder is loaded from its own directory exactly as the bridge loads a
//! snapshot (its `init.luau` beside its own byte-identical copy of the SDK),
//! with this test playing the host side of `plugin.<id>.imgui`. What is
//! asserted is what the contract's rows need before they can run: the
//! manifest, the windows each fixture asks for (id, title, size, position and
//! options), the probe actions, the tools, the per-window counters, the timer
//! and the store keys.
//!
//! The real windows, their pixels and the prints in the app's log are the
//! native checks (`ZERON_ZGX_DEV_PLUGINS` + `ZERON_ZGX_PLUGIN_LOG=1`); this
//! test is the protocol half, with no desktop.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value as Json;
use zgx_script::testing::TestApp;
use zgx_script::{Plugin, ScriptHost, Value};

use crate::plugins::imgui::frame::FrameCalls;
use crate::plugins::imgui::luau;
use crate::plugins::vlist;

/// Every fixture this feature authors, and the SDK copy each must carry.
const FIXTURES: &[&str] = &[
    "imgui-win-probe",
    "imgui-win-probe-b",
    "imgui-win-probe-broken-open",
    "imgui-no-permission",
    "imgui-win-probe-noperm",
    "imgui-win-probe-legacy",
    "imgui-custom-chrome",
    "imgui-contract-draw",
    "imgui-contract-style",
    "imgui-contract-style-b",
];

/// The main probe's launch set, in the order its `init.luau` opens them: five
/// windows so three slots stay free under the canonical 8-window cap, and
/// every other contract window opens via a probe action with no close first.
const LAUNCH: &[&str] = &["a", "b", "inspector", "bare", "actions"];

/// Every probe action the main fixture advertises, in its `init.luau` order.
/// Each must be an item in the one "Probe Actions" menu bar menu, titled by
/// the action's own name, and callable through `probe_run`.
const ACTIONS: &[&str] = &[
    "reopen",
    "blow_up",
    "close_inspector",
    "inspector_as_a",
    "inspector_unnamed",
    "inspector_700",
    "rename_inspector",
    "resize_inspector",
    "bump_inspector",
    "hide_inspector",
    "show_inspector",
    "focus_inspector",
    "minimize_inspector",
    "restore_inspector",
    "open_bounds",
    "open_drag",
    "open_quiet",
    "open_size",
    "open_pos",
    "open_pos2",
    "open_left",
    "open_right",
    "open_both_sides",
    "open_dup",
    "open_reuse",
    "reuse_cycle",
    "n_loop",
    "n_more",
    "close_all_windows",
    "open_a",
    "open_b",
    "open_c",
];

fn folder(id: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("plugins-dev")
        .join(id)
}

fn sdk_here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("plugins-sdk")
        .join("zeron.luau")
}

fn manifest_of(id: &str) -> Json {
    let text = std::fs::read_to_string(folder(id).join("zeron-plugin.json")).expect("its manifest");
    serde_json::from_str(&text).expect("a manifest")
}

fn permissions(id: &str) -> Vec<String> {
    manifest_of(id)["permissions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|permission| permission.as_str().map(str::to_string))
        .collect()
}

fn tools(id: &str) -> Vec<String> {
    manifest_of(id)["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect()
}

fn json_of(value: &Value) -> Json {
    serde_json::from_str(&value.to_json()).unwrap_or(Json::Null)
}

/// The fake host: what `imgui::luau` would answer, plus the caps the real
/// `Windows` enforces (a duplicate id, the live count of 8) so the fixture's
/// own actions are exercised under the same rules.
#[derive(Clone, Default)]
struct Fake {
    /// Every `imgui` op the plugin asked, parsed, in order.
    ops: Rc<RefCell<Vec<Json>>>,
    /// The `open_window` requests that were granted, parsed.
    opened: Rc<RefCell<Vec<Json>>>,
    /// The ids the fake host currently holds open.
    live: Rc<RefCell<Vec<String>>>,
    /// Each `plugin.<id>.menus` publication.
    menus: Rc<RefCell<Vec<Json>>>,
    /// Each `plugin.<id>.result` (a tool's answer).
    results: Rc<RefCell<Vec<Json>>>,
    /// Each `plugin.<id>.imgui_end`.
    ends: Rc<RefCell<Vec<Json>>>,
    /// Each `plugin.<id>.store_set`.
    stores: Rc<RefCell<Vec<Json>>>,
    /// How many `open_window` calls to refuse before answering, as a plugin
    /// missing `imgui.windows` is refused.
    refusals_left: Rc<Cell<usize>>,
    /// What a `button` answers: true is a release frame.
    pressed: Rc<Cell<bool>>,
    /// When set, only this label's button answers true (the pointer is over
    /// exactly one button); when unset, every button answers `pressed`, which
    /// is what the simple probe windows' single Ping button needs.
    pressed_label: Rc<RefCell<Option<String>>>,
}

fn ok() -> Value {
    Value::record([("ok", Value::Bool(true))])
}

fn failed(error: &str) -> Value {
    Value::record([("ok", Value::Bool(false)), ("error", Value::from(error))])
}

impl Fake {
    fn answer(&self, plugin: &str, asked: &Json) -> Value {
        self.ops.borrow_mut().push(asked.clone());
        match asked["op"].as_str().unwrap_or_default() {
            "open_window" => self.open(plugin, asked),
            "close" => {
                let id = asked["window"].as_str().unwrap_or_default().to_string();
                self.live.borrow_mut().retain(|open| *open != id);
                ok()
            }
            "is_open" => Value::record([
                ("ok", Value::Bool(true)),
                (
                    "open",
                    Value::Bool(self.is_open(asked["window"].as_str().unwrap_or_default())),
                ),
            ]),
            "state" => {
                let id = asked["window"].as_str().unwrap_or_default();
                Value::record([
                    ("ok", Value::Bool(true)),
                    ("id", Value::from(id)),
                    ("open", Value::Bool(self.is_open(id))),
                    ("shown", Value::Bool(self.is_open(id))),
                    ("minimized", Value::Bool(false)),
                ])
            }
            // The release frame: the fixture's branch runs then and only then.
            "button" => {
                let pressed = match &*self.pressed_label.borrow() {
                    Some(want) => asked["label"].as_str() == Some(want.as_str()),
                    None => self.pressed.get(),
                };
                Value::record([("ok", Value::Bool(true)), ("value", Value::Bool(pressed))])
            }
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
            _ => ok(),
        }
    }

    fn open(&self, plugin: &str, asked: &Json) -> Value {
        if self.refusals_left.get() > 0 {
            self.refusals_left.set(self.refusals_left.get() - 1);
            return failed(
                "This plugin wasn't allowed to open native application windows. Allow \"imgui.windows\" for it in Zeron.",
            );
        }
        let id = asked["id"].as_str().unwrap_or_default().to_string();
        {
            let live = self.live.borrow();
            if live.contains(&id) {
                return failed(&format!(
                    "Window \"{id}\" of plugin \"{plugin}\" is already open. Close it before opening it again."
                ));
            }
            if live.len() >= 8 {
                return failed(&format!(
                    "Plugin \"{plugin}\" can have 8 native windows open at once. Close one before opening another."
                ));
            }
        }
        self.live.borrow_mut().push(id.clone());
        self.opened.borrow_mut().push(asked.clone());
        Value::record([
            ("ok", Value::Bool(true)),
            ("id", Value::from(id.as_str())),
            ("open", Value::Bool(true)),
        ])
    }

    fn is_open(&self, id: &str) -> bool {
        self.live.borrow().iter().any(|open| open == id)
    }
}

/// A `TestApp` with fixture `id` loaded as plugin `id`, its manifest's
/// capabilities granted (as the dev path grants them), and a fake host.
fn harness(id: &str, refusals: usize) -> (TestApp, Fake) {
    let entry = folder(id).join("init.luau");
    let granted = crate::plugins::grants::capabilities(id, &permissions(id), &[], &[]);
    let fake = Fake::default();
    fake.refusals_left.set(refusals);
    let host_side = fake.clone();
    let plugin = id.to_string();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        let prefix = format!("plugin.{plugin}");
        for (key, _) in crate::plugins::grants::SHARED_READ {
            host.expose_shared_read(*key, None);
            host.shared(key, Value::Nil);
        }
        for name in crate::plugins::grants::OWN_SHARED {
            host.expose_shared_read(format!("{prefix}.{name}"), None);
            let empty = match *name {
                "calls" | "inbox" | "menu_items" | "locales" | "composed" => vlist(Vec::new()),
                _ => Value::record(std::iter::empty::<(&str, Value)>()),
            };
            host.shared(&format!("{prefix}.{name}"), Value::Nil)
                .set(empty);
        }
        host.expose_action(format!("{prefix}.imgui"), None);
        host.expose_action(format!("{prefix}.imgui_end"), None);
        for name in crate::plugins::grants::OWN_ACTIONS {
            host.expose_action(format!("{prefix}.{name}"), None);
        }
        let answer = host_side.clone();
        let answer_plugin = plugin.clone();
        host.function(format!("{prefix}.imgui"), move |arg| {
            let asked = json_of(&arg);
            answer.answer(&answer_plugin, &asked)
        });
        let ended = host_side.clone();
        host.action(format!("{prefix}.imgui_end"), move |arg| {
            ended.ends.borrow_mut().push(json_of(&arg));
        });
        let menus = host_side.clone();
        host.action(format!("{prefix}.menus"), move |arg| {
            menus.menus.borrow_mut().push(json_of(&arg));
        });
        let results = host_side.clone();
        host.action(format!("{prefix}.result"), move |arg| {
            results.results.borrow_mut().push(json_of(&arg));
        });
        let stores = host_side.clone();
        host.action(format!("{prefix}.store_set"), move |arg| {
            stores.stores.borrow_mut().push(json_of(&arg));
        });
        host.set_plugins(vec![Plugin {
            name: plugin.clone(),
            entry: entry.clone(),
            grants: granted.clone(),
        }]);
        host
    });
    (app, fake)
}

/// The ops of one kind the fake host heard.
fn ops(fake: &Fake, op: &str) -> Vec<Json> {
    fake.ops
        .borrow()
        .iter()
        .filter(|asked| asked["op"].as_str() == Some(op))
        .cloned()
        .collect()
}

/// The granted `open_window` requests for one id.
fn opened(fake: &Fake, id: &str) -> Vec<Json> {
    fake.opened
        .borrow()
        .iter()
        .filter(|spec| spec["id"].as_str() == Some(id))
        .cloned()
        .collect()
}

fn number(value: &Json) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

/// Pushes one frame request into the plugin's queue, as the pump does, and
/// lets the plugin run it.
fn push_frame(app: &TestApp, calls: &FrameCalls, window: &str, seq: f64) {
    let id = format!("imgui-frame-{window}-{seq}");
    calls.push(
        id.clone(),
        "imgui.frame",
        vec![
            ("window", Value::from(window)),
            ("seq", Value::from(seq)),
            ("budget", Value::from(8.0)),
        ],
    );
    app.frames(3);
    calls.take(&id);
    app.frames(1);
}

/// Runs one of the fixture's tools and returns its answer record
/// (`{ id, ok, value | error }`).
fn run_tool(app: &TestApp, fake: &Fake, calls: &FrameCalls, tool: &str, args: Json) -> Json {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let id = format!("probe-call-{tool}-{n}");
    calls.push(
        id.clone(),
        "tool",
        vec![
            ("tool", Value::from(tool)),
            ("args", crate::plugins::json_value(&args)),
        ],
    );
    app.frames(6);
    calls.take(&id);
    app.frames(3);
    fake.results
        .borrow()
        .iter()
        .rev()
        .find(|answer| answer["id"].as_str() == Some(id.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("{tool} did not answer: {:?}", fake.results.borrow()))
}

/// Runs one probe action through the fixture's `probe_run` tool.
fn run_action(app: &TestApp, fake: &Fake, calls: &FrameCalls, action: &str) -> Json {
    run_tool(
        app,
        fake,
        calls,
        "probe_run",
        serde_json::json!({ "action": action }),
    )
}

fn status(app: &TestApp, fake: &Fake, calls: &FrameCalls) -> Json {
    let answer = run_tool(app, fake, calls, "probe_status", Json::Null);
    assert_eq!(
        answer["ok"],
        Json::Bool(true),
        "probe_status failed: {answer}"
    );
    answer["value"].clone()
}

/// A fixture loaded to steady state with its frame queue, ready to drive one
/// probe action from the five launch windows (so its three free slots absorb
/// the action without a close first).
fn driven(id: &str) -> (TestApp, Fake, FrameCalls) {
    let (app, fake) = harness(id, 0);
    app.frames(30);
    let (calls, _closed) = luau::dev_listen(&app.host, id, |_| {});
    (app, fake, calls)
}

/// Every fixture folder carries the shipped SDK, byte for byte: a stale copy
/// is a plugin that behaves like an older SDK.
#[test]
fn every_fixture_carries_the_shipped_sdk() {
    let shipped = std::fs::read(sdk_here()).expect("plugins-sdk/zeron.luau");
    for id in FIXTURES {
        let copied = std::fs::read(folder(id).join("zeron.luau"))
            .unwrap_or_else(|_| panic!("plugins-dev/{id}/zeron.luau is missing"));
        assert_eq!(
            copied, shipped,
            "plugins-dev/{id}/zeron.luau is not the shipped SDK: copy plugins-sdk/zeron.luau over it"
        );
        assert!(
            folder(id).join("README.md").is_file(),
            "plugins-dev/{id}/README.md names the rows it serves and how to drive it"
        );
    }
}

/// Each manifest says what its rows exercise, and asks for `imgui.windows`
/// exactly where it must.
#[test]
fn every_fixture_declares_the_manifest_the_rows_expect() {
    let main = manifest_of("imgui-win-probe");
    assert_eq!(main["id"], Json::from("imgui-win-probe"), "{main}");
    assert_eq!(main["entry"], Json::from("init.luau"), "{main}");
    assert!(
        permissions("imgui-win-probe")
            .iter()
            .any(|p| p == "imgui.windows"),
        "the main probe asks for imgui.windows: {main}"
    );
    assert!(
        tools("imgui-win-probe").iter().any(|t| t == "probe_status"),
        "the main probe offers probe_status: {main}"
    );
    assert!(
        tools("imgui-win-probe").iter().any(|t| t == "probe_run"),
        "the main probe offers probe_run: {main}"
    );

    let second = manifest_of("imgui-win-probe-b");
    assert_eq!(second["id"], Json::from("imgui-win-probe-b"), "{second}");
    assert!(
        permissions("imgui-win-probe-b")
            .iter()
            .any(|p| p == "imgui.windows"),
        "{second}"
    );
    assert!(
        tools("imgui-win-probe-b")
            .iter()
            .any(|t| t == "probe_status"),
        "{second}"
    );

    let broken = manifest_of("imgui-win-probe-broken-open");
    assert_eq!(
        broken["id"],
        Json::from("imgui-win-probe-broken-open"),
        "{broken}"
    );
    assert!(
        permissions("imgui-win-probe-broken-open")
            .iter()
            .any(|p| p == "imgui.windows"),
        "{broken}"
    );
    assert!(
        tools("imgui-win-probe-broken-open")
            .iter()
            .any(|t| t == "probe_status"),
        "{broken}"
    );

    // The consent fixture asks for no imgui.windows until the row adds it.
    let consent = permissions("imgui-no-permission");
    assert!(
        !consent.iter().any(|p| p == "imgui.windows"),
        "imgui-no-permission must not ask: {consent:?}"
    );

    // VAL-WIN-010's two variants: nothing, and the legacy permission alone.
    assert!(
        permissions("imgui-win-probe-noperm").is_empty(),
        "imgui-win-probe-noperm declares no permissions"
    );
    assert_eq!(
        permissions("imgui-win-probe-legacy"),
        vec!["windows".to_string()],
        "imgui-win-probe-legacy declares the declarative windows permission alone"
    );
}

/// The launch windows the contract needs at once, with the exact specs its
/// rows measure, and nothing else until an action asks.
#[test]
fn the_main_probe_opens_the_launch_windows_the_rows_need() {
    let (app, fake) = harness("imgui-win-probe", 0);
    app.frames(30);
    assert_eq!(
        app.error(),
        None,
        "the fixture did not load: {:?}",
        app.error()
    );

    let granted = fake.opened.borrow().clone();
    let ids: Vec<&str> = granted
        .iter()
        .filter_map(|spec| spec["id"].as_str())
        .collect();
    assert_eq!(ids, LAUNCH, "the launch set, in order: {granted:?}");

    // VAL-WIN-001: the inspector, titled byte for byte and 720x480.
    let inspector = &opened(&fake, "inspector")[0];
    assert_eq!(
        inspector["title"],
        Json::from("Probe Inspector"),
        "{inspector}"
    );
    assert_eq!(number(&inspector["size"]["width"]), 720.0, "{inspector}");
    assert_eq!(number(&inspector["size"]["height"]), 480.0, "{inspector}");
    assert_eq!(
        inspector["remember"],
        Json::Bool(true),
        "restored geometry: {inspector}"
    );

    // VAL-WIN-005: two windows from one script, with distinct titles; the
    // third (`c`) is one probe action away under the same row.
    assert_eq!(opened(&fake, "a")[0]["title"], Json::from("Probe A"));
    assert_eq!(opened(&fake, "b")[0]["title"], Json::from("Probe B"));

    // VAL-WIN-019: no platform title bar at all.
    let bare = &opened(&fake, "bare")[0];
    assert_eq!(bare["title"], Json::from("Probe Bare"), "{bare}");
    assert_eq!(number(&bare["size"]["width"]), 600.0, "{bare}");
    assert_eq!(number(&bare["size"]["height"]), 400.0, "{bare}");
    assert_eq!(bare["titlebar"], Json::from("hidden"), "{bare}");
    assert_eq!(bare["decorations"], Json::Bool(false), "{bare}");

    // The fifth launch window is the pointer-drive surface the rows reach
    // without Zeron's menu bar: sized so all 32 buttons fit without scrolling.
    let actions = &opened(&fake, "actions")[0];
    assert_eq!(actions["title"], Json::from("Probe Actions"), "{actions}");
    assert_eq!(number(&actions["size"]["width"]), 420.0, "{actions}");
    assert_eq!(number(&actions["size"]["height"]), 1000.0, "{actions}");

    // Every launch window asked for a render callback, so the pump has one
    // to push frames into.
    let renders = ops(&fake, "render");
    assert_eq!(renders.len(), LAUNCH.len(), "{renders:?}");
    for render in &renders {
        assert_eq!(render["on"], Json::Bool(true), "{render}");
        assert!(
            LAUNCH.contains(&render["window"].as_str().unwrap_or_default()),
            "a launch window's render: {render}"
        );
    }

    // Nothing else opens unasked: the rest of the contract's windows wait for
    // a probe action (the per-plugin cap is 8 and three slots stay free).
    for extra in [
        "bounds", "c", "drag", "quiet", "left", "right", "dup", "reuse", "size", "pos", "pos2",
        "n1",
    ] {
        assert!(
            opened(&fake, extra).is_empty(),
            "{extra} opened without an action: {:?}",
            fake.opened.borrow()
        );
    }

    // VAL-LIFE-016: the quiet preference is recorded under its own key.
    let stores = fake.stores.borrow().clone();
    let quiet = stores
        .iter()
        .find(|set| set["key"].as_str() == Some("imgui.windows.quiet.visible"))
        .unwrap_or_else(|| panic!("no quiet preference was written: {stores:?}"));
    assert_eq!(quiet["value"], Json::Bool(false), "{quiet}");
    assert!(
        stores
            .iter()
            .any(|set| set["key"].as_str() == Some("imgui.windows.inspector.prefs")),
        "inspector prefs: {stores:?}"
    );
    assert!(
        stores
            .iter()
            .any(|set| set["key"].as_str() == Some("imgui.windows.drag.prefs")),
        "drag prefs: {stores:?}"
    );
}

/// The fifth launch window drives every advertised action from a button: one
/// button per action, labelled with its own name, whose press runs the same
/// `run_action` the menu and `probe_run` call, deferred until the frame is
/// over so opening a window is allowed.
#[test]
fn the_actions_window_draws_and_drives_every_advertised_action() {
    let (app, fake, calls) = driven("imgui-win-probe");

    // One frame of the actions window draws a button per advertised action,
    // in the advertised order.
    push_frame(&app, &calls, "actions", 1.0);
    let labels: Vec<String> = ops(&fake, "button")
        .into_iter()
        .filter_map(|asked| asked["label"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        labels, ACTIONS,
        "one button per advertised action, in order"
    );

    // Pressing the open_bounds button runs the same open_bounds action the
    // menu and the tool run: after the deferred call, bounds opens at its
    // measured spec with nothing closed first.
    *fake.pressed_label.borrow_mut() = Some("open_bounds".to_string());
    push_frame(&app, &calls, "actions", 2.0);
    app.frames(30);
    let bounds = opened(&fake, "bounds");
    assert_eq!(
        bounds.len(),
        1,
        "the open_bounds button opened bounds: {bounds:?}"
    );
    let spec = &bounds[0];
    assert_eq!(number(&spec["size"]["width"]), 400.0, "{spec}");
    assert_eq!(number(&spec["size"]["height"]), 300.0, "{spec}");
    assert_eq!(number(&spec["min_size"]["width"]), 400.0, "{spec}");
    assert_eq!(number(&spec["min_size"]["height"]), 300.0, "{spec}");
    assert_eq!(number(&spec["max_size"]["width"]), 900.0, "{spec}");
    assert_eq!(number(&spec["max_size"]["height"]), 600.0, "{spec}");
    assert_eq!(spec["resizable"], Json::Bool(true), "{spec}");
    assert!(
        ops(&fake, "close").is_empty(),
        "the actions window closed a window first: {:?}",
        fake.ops.borrow()
    );
}

/// The probe actions the rows drive: each opens (or changes) the window the
/// row names, with that row's options, and the one-action-away windows open
/// without closing anything first.
#[test]
fn the_main_probe_offers_the_probe_actions_the_rows_drive() {
    // Every advertised action is an item in the one "Probe Actions" menu bar
    // menu, titled by the action's own name, at its `<plugin>/<action>` id;
    // `probe_run` is the tool route to the same table.
    let (app, _fake) = harness("imgui-win-probe", 0);
    app.frames(30);
    let probe_menus: Vec<(String, String)> = app
        .host
        .commands()
        .get()
        .into_iter()
        .filter(|command| command.menu == "Probe Actions")
        .map(|command| (command.id.clone(), command.title.clone()))
        .collect();
    for action in ACTIONS {
        assert!(
            probe_menus
                .iter()
                .any(|(id, title)| id == &format!("imgui-win-probe/{action}") && title == *action),
            "no Probe Actions item for {action}: {probe_menus:?}"
        );
    }

    // VAL-WIN-004: the bounds window, both bounds stated, no close first.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_bounds");
        let bounds = &opened(&fake, "bounds")[0];
        assert_eq!(number(&bounds["min_size"]["width"]), 400.0, "{bounds}");
        assert_eq!(number(&bounds["min_size"]["height"]), 300.0, "{bounds}");
        assert_eq!(number(&bounds["max_size"]["width"]), 900.0, "{bounds}");
        assert_eq!(number(&bounds["max_size"]["height"]), 600.0, "{bounds}");
        assert_eq!(bounds["resizable"], Json::Bool(true), "{bounds}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_bounds closed a window first: {:?}",
            fake.ops.borrow()
        );
    }

    // VAL-WIN-019...022: hidden chrome and the plugin's own bar.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_drag");
        let drag = &opened(&fake, "drag")[0];
        assert_eq!(drag["title"], Json::from("Probe Drag"), "{drag}");
        assert_eq!(drag["titlebar"], Json::from("hidden"), "{drag}");
        assert_eq!(drag["decorations"], Json::Bool(false), "{drag}");
        assert_eq!(drag["resizable"], Json::Bool(true), "{drag}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_drag closed a window first: {:?}",
            fake.ops.borrow()
        );
    }

    // VAL-WIN-002/003: size, pos and pos2 are one action away, with the
    // exact geometry each row measures and no close first.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_size");
        let size = &opened(&fake, "size")[0];
        assert_eq!(size["title"], Json::from("Probe Size"), "{size}");
        assert_eq!(number(&size["size"]["width"]), 720.0, "{size}");
        assert_eq!(number(&size["size"]["height"]), 480.0, "{size}");
        assert_eq!(size["resizable"], Json::Bool(true), "{size}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_size closed a window first: {:?}",
            fake.ops.borrow()
        );
    }
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_pos");
        let pos = &opened(&fake, "pos")[0];
        assert_eq!(pos["title"], Json::from("Probe Pos"), "{pos}");
        assert_eq!(number(&pos["size"]["width"]), 400.0, "{pos}");
        assert_eq!(number(&pos["size"]["height"]), 300.0, "{pos}");
        assert_eq!(number(&pos["position"]["x"]), 100.0, "{pos}");
        assert_eq!(number(&pos["position"]["y"]), 120.0, "{pos}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_pos closed a window first: {:?}",
            fake.ops.borrow()
        );
    }
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_pos2");
        let pos2 = &opened(&fake, "pos2")[0];
        assert_eq!(pos2["title"], Json::from("Probe Pos Two"), "{pos2}");
        assert_eq!(number(&pos2["size"]["width"]), 400.0, "{pos2}");
        assert_eq!(number(&pos2["size"]["height"]), 300.0, "{pos2}");
        assert_eq!(number(&pos2["position"]["x"]), 600.0, "{pos2}");
        assert_eq!(number(&pos2["position"]["y"]), 420.0, "{pos2}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_pos2 closed a window first: {:?}",
            fake.ops.borrow()
        );
    }

    // VAL-LIFE-016: quiet opens hidden, from its recorded preference.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_quiet");
        let quiet = &opened(&fake, "quiet")[0];
        assert_eq!(quiet["title"], Json::from("Probe Quiet"), "{quiet}");
        assert_eq!(quiet["visible"], Json::Bool(false), "{quiet}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_quiet closed a window first: {:?}",
            fake.ops.borrow()
        );
    }

    // VAL-LIFE-009: each side at its own size, remembered, and one action
    // away; `open_both_sides` opens the two at once.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_left");
        let left = &opened(&fake, "left")[0];
        assert_eq!(left["title"], Json::from("Probe Left"), "{left}");
        assert_eq!(number(&left["size"]["width"]), 500.0, "{left}");
        assert_eq!(number(&left["size"]["height"]), 400.0, "{left}");
        assert_eq!(left["remember"], Json::Bool(true), "{left}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_left closed a window first: {:?}",
            fake.ops.borrow()
        );
    }
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_right");
        let right = &opened(&fake, "right")[0];
        assert_eq!(right["title"], Json::from("Probe Right"), "{right}");
        assert_eq!(number(&right["size"]["width"]), 900.0, "{right}");
        assert_eq!(number(&right["size"]["height"]), 700.0, "{right}");
        assert_eq!(right["remember"], Json::Bool(true), "{right}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_right closed a window first: {:?}",
            fake.ops.borrow()
        );
    }
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_both_sides");
        let left = &opened(&fake, "left")[0];
        assert_eq!(left["remember"], Json::Bool(true), "{left}");
        let right = &opened(&fake, "right")[0];
        assert_eq!(right["remember"], Json::Bool(true), "{right}");
        assert!(
            ops(&fake, "close").is_empty(),
            "open_both_sides closed a window first: {:?}",
            fake.ops.borrow()
        );
    }

    // VAL-WIN-007: dup opens once and the second request is refused with its
    // id in the sentence, which the fixture keeps and reports.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_dup");
        assert_eq!(opened(&fake, "dup").len(), 1, "{:?}", fake.opened.borrow());
        let dup_asks: Vec<Json> = ops(&fake, "open_window")
            .into_iter()
            .filter(|asked| asked["id"].as_str() == Some("dup"))
            .collect();
        assert_eq!(
            dup_asks.len(),
            2,
            "the duplicate was attempted: {dup_asks:?}"
        );
        assert_eq!(
            dup_asks[1]["title"],
            Json::from("Second Dup"),
            "{dup_asks:?}"
        );
        let after = status(&app, &fake, &calls);
        assert!(
            after["last_refusal"]
                .as_str()
                .unwrap_or_default()
                .contains("dup"),
            "the refusal names the id: {after}"
        );
        assert!(
            ops(&fake, "close").is_empty(),
            "open_dup closed a window first: {:?}",
            fake.ops.borrow()
        );
    }

    // VAL-WIN-008: the reuse id opens plain, then closes and opens again.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "open_reuse");
        assert_eq!(
            opened(&fake, "reuse")[0]["title"],
            Json::from("Probe Reuse")
        );
        assert!(
            ops(&fake, "close").is_empty(),
            "open_reuse closed a window first: {:?}",
            fake.ops.borrow()
        );
        run_action(&app, &fake, &calls, "reuse_cycle");
        app.frames(25);
        let reuse = opened(&fake, "reuse");
        assert_eq!(reuse.len(), 2, "the reopen: {reuse:?}");
        assert_eq!(
            reuse[1]["title"],
            Json::from("Probe Reuse Again"),
            "{reuse:?}"
        );
    }

    // VAL-WIN-011/012: the live window's controls and setters, all against
    // the inspector that opened at launch.
    {
        let (app, fake, calls) = driven("imgui-win-probe");
        run_action(&app, &fake, &calls, "hide_inspector");
        run_action(&app, &fake, &calls, "show_inspector");
        run_action(&app, &fake, &calls, "focus_inspector");
        run_action(&app, &fake, &calls, "minimize_inspector");
        run_action(&app, &fake, &calls, "restore_inspector");
        run_action(&app, &fake, &calls, "rename_inspector");
        run_action(&app, &fake, &calls, "resize_inspector");
        run_action(&app, &fake, &calls, "inspector_as_a");
        run_action(&app, &fake, &calls, "inspector_unnamed");
        for op in ["hide", "show", "focus", "minimize", "restore"] {
            assert!(
                ops(&fake, op)
                    .iter()
                    .any(|asked| asked["window"].as_str() == Some("inspector")),
                "the {op} control reached the inspector"
            );
        }
        let titles: Vec<String> = ops(&fake, "set_title")
            .iter()
            .filter_map(|asked| asked["title"].as_str().map(str::to_string))
            .collect();
        for title in ["Renamed Probe", "A Inspector", "Inspector"] {
            assert!(titles.iter().any(|said| said == title), "{titles:?}");
        }
        assert!(
            ops(&fake, "set_size")
                .iter()
                .any(|asked| number(&asked["size"]["width"]) == 1000.0
                    && number(&asked["size"]["height"]) == 640.0),
            "resize_inspector"
        );
        assert!(
            ops(&fake, "set_size")
                .iter()
                .any(|asked| number(&asked["size"]["width"]) == 400.0
                    && number(&asked["size"]["height"]) == 300.0),
            "inspector_as_a"
        );

        // VAL-LIFE-007: close then reopen by the same id.
        run_action(&app, &fake, &calls, "close_inspector");
        run_action(&app, &fake, &calls, "reopen");
        app.frames(30);
        assert!(
            opened(&fake, "inspector").len() >= 2,
            "reopened: {:?}",
            fake.opened.borrow()
        );
    }
}

/// The probe's own JSON: per-window frames and counters, the timer that keeps
/// changing, the store keys, and the plugin still answering with no window.
#[test]
fn the_main_probe_reports_per_window_state_ticks_and_store() {
    let (app, fake) = harness("imgui-win-probe", 0);
    app.frames(30);
    let calls = luau::dev_listen(&app.host, "imgui-win-probe", |_| {}).0;

    // One frame of a, then a release frame of a: only a's counter moves.
    push_frame(&app, &calls, "a", 1.0);
    fake.pressed.set(true);
    push_frame(&app, &calls, "a", 2.0);
    fake.pressed.set(false);
    push_frame(&app, &calls, "b", 3.0);

    let first = status(&app, &fake, &calls);
    assert_eq!(first["plugin"], Json::from("imgui-win-probe"), "{first}");
    assert_eq!(first["version"], Json::from("1"), "{first}");
    assert_eq!(number(&first["counters"]["a"]), 1.0, "{first}");
    assert_eq!(number(&first["counters"]["b"]), 0.0, "{first}");
    assert!(number(&first["frames"]["a"]) >= 2.0, "{first}");
    assert!(number(&first["frames"]["b"]) >= 1.0, "{first}");
    let windows: Vec<&str> = first["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str())
        .collect();
    for id in LAUNCH {
        assert!(
            windows.contains(id),
            "{id} is in the status windows: {first}"
        );
    }
    assert_eq!(
        first["store"]["quiet_visible"],
        Json::Bool(false),
        "{first}"
    );
    // The two preferences are written at launch; the fake host does not
    // mirror writes back into the store it publishes, so what was written is
    // read from the writes themselves.
    let stores = fake.stores.borrow().clone();
    assert!(
        stores
            .iter()
            .any(|set| set["key"].as_str() == Some("imgui.windows.inspector.prefs")),
        "{stores:?}"
    );
    assert!(
        stores
            .iter()
            .any(|set| set["key"].as_str() == Some("imgui.windows.drag.prefs")),
        "{stores:?}"
    );

    // The timer keeps changing on its own (250 ms), with no button and no
    // probe action involved: the lifecycle rows watch this value.
    let before = number(&first["ticks"]);
    app.frames(30);
    let later = status(&app, &fake, &calls);
    assert!(
        number(&later["ticks"]) > before,
        "the timer did not advance: {before} -> {}",
        later["ticks"]
    );

    // The tool still answers with every window closed: VAL-LIFE-007's
    // condition that the plugin stays loaded is observable, not assumed.
    run_action(&app, &fake, &calls, "close_all_windows");
    fake.answer(
        "imgui-win-probe",
        &serde_json::json!({ "op": "close", "window": "actions" }),
    );
    app.frames(10);
    let closed = status(&app, &fake, &calls);
    assert_eq!(number(&closed["open"]), 0.0, "{closed}");
    assert!(
        closed["windows"]
            .as_array()
            .is_some_and(|list| list.is_empty()),
        "{closed}"
    );
    let before = number(&closed["ticks"]);
    app.frames(30);
    let still = status(&app, &fake, &calls);
    assert!(
        number(&still["ticks"]) > before,
        "the timer stops with no windows: {before} -> {}",
        still["ticks"]
    );
}

/// `VAL-WIN-013`/`014`: a close runs `on_close` once, logs the line the row
/// counts, and retitles a sibling; a second delivery for the same id is inert.
#[test]
fn the_main_probe_closes_once_and_retitles_a_sibling() {
    let (app, fake) = harness("imgui-win-probe", 0);
    app.frames(30);
    let (_calls, closed) = luau::dev_listen(&app.host, "imgui-win-probe", |_| {});
    fake.ops.borrow_mut().clear();

    closed("b");
    app.frames(3);
    let titles = ops(&fake, "set_title");
    assert_eq!(titles.len(), 1, "one sibling retitled: {titles:?}");
    assert_eq!(titles[0]["window"], Json::from("a"), "{titles:?}");
    assert_eq!(titles[0]["title"], Json::from("closed 1"), "{titles:?}");

    // The SDK marks every handle for the closed id inert: a second delivery
    // must run the callback no second time.
    closed("b");
    app.frames(3);
    assert_eq!(
        ops(&fake, "set_title").len(),
        1,
        "on_close ran twice: {:?}",
        ops(&fake, "set_title")
    );
}

/// `VAL-WIN-009`: the live cap is reached, named, and freed by a close.
#[test]
fn the_main_probe_reaches_the_cap_and_reuses_a_slot() {
    let (app, fake) = harness("imgui-win-probe", 0);
    app.frames(30);
    let calls = luau::dev_listen(&app.host, "imgui-win-probe", |_| {}).0;

    run_action(&app, &fake, &calls, "n_loop");
    app.frames(30);
    let at_cap = status(&app, &fake, &calls);
    assert_eq!(
        number(&at_cap["open"]),
        8.0,
        "the loop stopped at 8: {at_cap}"
    );
    assert!(
        at_cap["last_refusal"]
            .as_str()
            .unwrap_or_default()
            .contains('8'),
        "the refusal names the limit: {at_cap}"
    );

    run_action(&app, &fake, &calls, "n_more");
    app.frames(30);
    let again = status(&app, &fake, &calls);
    assert_eq!(number(&again["open"]), 8.0, "close one, open one: {again}");
}

/// `VAL-WIN-006`/`VAL-LIFE-009`: the same window id in two plugins is two
/// native windows with their own geometry.
#[test]
fn the_second_probe_opens_the_same_id_at_its_own_geometry() {
    let (main, main_fake) = harness("imgui-win-probe", 0);
    main.frames(30);
    assert_eq!(main.error(), None);
    let a = opened(&main_fake, "inspector")[0].clone();

    let (second, second_fake) = harness("imgui-win-probe-b", 0);
    second.frames(30);
    assert_eq!(second.error(), None, "{:?}", second.error());
    let granted = second_fake.opened.borrow().clone();
    assert_eq!(granted.len(), 1, "B opens one window: {granted:?}");
    let b = &granted[0];
    assert_eq!(b["id"], Json::from("inspector"), "{b}");
    assert_eq!(b["title"], Json::from("B Inspector"), "{b}");
    assert_eq!(number(&b["size"]["width"]), 700.0, "{b}");
    assert_eq!(number(&b["size"]["height"]), 500.0, "{b}");
    assert_eq!(number(&b["position"]["x"]), 500.0, "{b}");
    assert_eq!(number(&b["position"]["y"]), 300.0, "{b}");
    assert_eq!(b["remember"], Json::Bool(true), "{b}");

    // Neither plugin can be mistaken for the other: same id, different title,
    // different size, different origin.
    assert_ne!(a["title"], b["title"], "{a} / {b}");
    assert_ne!(a["size"], b["size"], "{a} / {b}");
    assert_ne!(a["position"], b["position"], "{a} / {b}");

    // B's tool answers on its own, and B closes its own window. The host
    // tells the plugin of the close on its call queue, which the test plays.
    let (calls, closed) = luau::dev_listen(&second.host, "imgui-win-probe-b", |_| {});
    run_action(&second, &second_fake, &calls, "close_inspector");
    closed("inspector");
    second.frames(3);
    let after = status(&second, &second_fake, &calls);
    assert_eq!(number(&after["open"]), 0.0, "{after}");
    assert_eq!(number(&after["closed"]), 1.0, "{after}");
}

/// `VAL-LIFE-007`: the fixture's window cannot draw its first frame; the
/// frame must be reported as failed, with the row's message, and nothing
/// drawn.
#[test]
fn the_broken_open_fixture_fails_its_first_frame() {
    let (app, fake) = harness("imgui-win-probe-broken-open", 0);
    app.frames(30);
    assert_eq!(
        app.error(),
        None,
        "the plugin itself loads: {:?}",
        app.error()
    );
    let granted = fake.opened.borrow().clone();
    assert_eq!(granted.len(), 1, "{granted:?}");
    assert_eq!(granted[0]["id"], Json::from("broken"), "{granted:?}");
    assert_eq!(
        granted[0]["title"],
        Json::from("Probe Broken Open"),
        "{granted:?}"
    );

    let calls = luau::dev_listen(&app.host, "imgui-win-probe-broken-open", |_| {}).0;
    push_frame(&app, &calls, "broken", 1.0);
    let ends = fake.ends.borrow().clone();
    let end = ends
        .last()
        .unwrap_or_else(|| panic!("no frame ended: {ends:?}"));
    assert_eq!(end["window"], Json::from("broken"), "{end}");
    assert_eq!(end["ok"], Json::Bool(false), "{end}");
    assert!(
        end["error"]
            .as_str()
            .unwrap_or_default()
            .contains("probe open failure"),
        "{end}"
    );
    assert!(
        ops(&fake, "text").is_empty(),
        "nothing was drawn: {:?}",
        ops(&fake, "text")
    );
}

/// The consent fixture asks again until the host allows it, then opens and
/// draws without a restart.
#[test]
fn the_no_permission_fixture_asks_until_it_is_allowed() {
    let (app, fake) = harness("imgui-no-permission", 3);
    // Three refusals then an answer: the first ask at load, then one every
    // 500 ms. 120 frames is two seconds.
    app.frames(120);
    assert_eq!(
        app.error(),
        None,
        "a refusal is not a script error: {:?}",
        app.error()
    );

    let asks = ops(&fake, "open_window");
    assert!(asks.len() >= 4, "the fixture kept asking: {asks:?}");
    assert!(
        asks.iter()
            .all(|asked| asked["id"].as_str() == Some("consent")),
        "{asks:?}"
    );
    let granted = opened(&fake, "consent");
    assert_eq!(granted.len(), 1, "one window once allowed: {granted:?}");
    assert_eq!(
        granted[0]["title"],
        Json::from("No Permission Probe"),
        "{granted:?}"
    );
    assert_eq!(number(&granted[0]["size"]["width"]), 480.0, "{granted:?}");
    assert_eq!(number(&granted[0]["size"]["height"]), 320.0, "{granted:?}");
    assert!(
        ops(&fake, "render")
            .iter()
            .any(|render| render["window"].as_str() == Some("consent")),
        "it draws once allowed"
    );

    let calls = luau::dev_listen(&app.host, "imgui-no-permission", |_| {}).0;
    push_frame(&app, &calls, "consent", 1.0);
    let drawn: Vec<String> = ops(&fake, "text")
        .iter()
        .filter_map(|asked| asked["text"].as_str().map(str::to_string))
        .collect();
    assert!(
        drawn.iter().any(|line| line == "No Permission Probe"),
        "the allowed window drew: {drawn:?}"
    );
}

/// `VAL-WIN-010` (a) and (b): no permission, and the legacy permission
/// alone, are both refused the native window and both stay loaded.
#[test]
fn the_noperm_and_legacy_fixtures_are_refused_and_stay_loaded() {
    let (noperm, noperm_fake) = harness("imgui-win-probe-noperm", 1);
    noperm.frames(60);
    assert_eq!(
        noperm.error(),
        None,
        "a refusal is not a script error: {:?}",
        noperm.error()
    );
    let asks = ops(&noperm_fake, "open_window");
    assert_eq!(asks.len(), 1, "one ask: {asks:?}");
    assert_eq!(asks[0]["id"], Json::from("x"), "{asks:?}");
    assert_eq!(asks[0]["title"], Json::from("Denied"), "{asks:?}");
    assert!(noperm_fake.opened.borrow().is_empty(), "no window opened");

    let (legacy, legacy_fake) = harness("imgui-win-probe-legacy", 1);
    legacy.frames(60);
    assert_eq!(
        legacy.error(),
        None,
        "the declarative window loads: {:?}",
        legacy.error()
    );
    assert!(
        legacy.window("legacy-panel").is_some(),
        "the declarative ui.window panel is still declared and draws"
    );
    let asks = ops(&legacy_fake, "open_window");
    assert_eq!(asks.len(), 1, "one ask: {asks:?}");
    assert!(
        legacy_fake.opened.borrow().is_empty(),
        "no native window opened"
    );
}
