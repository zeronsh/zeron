//! The Rust side of the Orchestrators plugin (plugins-builtin/orchestrators),
//! on its switch: what feeds its screens runs only while it is on. Its four
//! parts, src/orchestration.rs (the state, comms and device loads),
//! src/orchestration_timeline.rs, src/orchestration_progress.rs and
//! src/scratchpad.rs, each keep a window's shared values; this module opens
//! their engine streams and fills those values while the plugin is on, and
//! while it's off opens none and gives the scripts nothing.
//!
//! The switch is `plugins.on`'s (src/plugins/switch.rs: on in the engine's
//! list and loaded in the window), followed live:
//! - on: each window's values set (loading at first), and the streams opened
//!   on the connection there is, and on every connection after;
//! - off: the streams closed, what they had kept let go of, and each value a
//!   window was given set to nil. A window that was never given one has none
//!   (`ui.shared`'s own default, as in an app without the plugin).
//!
//! Their actions (`orchestration.*`, `scratchpad.*`) answer "off" while it is.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, rpc::Rpc};
use crate::{orchestration, orchestration_progress, orchestration_timeline, scratchpad};

/// The plugin's id (its manifest's, and `plugins.on`'s key).
pub const ID: &str = "orchestrators";

/// Whether the plugin is on now.
pub fn on() -> bool {
    crate::plugins::switch::on(ID)
}

/// An action of the plugin's, refused while it's off.
pub fn refuse() -> Result<(), String> {
    if on() { Ok(()) } else { Err("off".to_string()) }
}

thread_local! {
    /// The switch and the engine followed (once, for every window).
    static FOLLOWING: Cell<bool> = const { Cell::new(false) };
}

/// The plugin's parts for the window `cx` shows `host` in.
pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let windows = add(host);
    cx.on_closed(move || remove(windows));
    orchestration::actions(host, windows.0);
    orchestration_progress::actions(host);
    scratchpad::actions(host);
    follow(|| engine::get().current());
}

/// Each part's id for one window.
#[derive(Clone, Copy)]
pub(crate) struct Windows(u64, u64, u64, u64);

/// `host`'s window, to each part: its values set now if the plugin is on.
pub(crate) fn add(host: &ScriptHost) -> Windows {
    Windows(
        orchestration::add(host),
        orchestration_timeline::add(host),
        orchestration_progress::add(host),
        scratchpad::add(host),
    )
}

fn remove(w: Windows) {
    orchestration::remove(w.0);
    orchestration_timeline::remove(w.1);
    orchestration_progress::remove(w.2);
    scratchpad::remove(w.3);
}

/// The switch and the engine's connections followed, once: `current` is the
/// connection there is now (the engine's; a test's own).
pub(crate) fn follow(current: impl Fn() -> Option<Rpc> + 'static) {
    if FOLLOWING.with(|f| f.replace(true)) {
        return;
    }
    let current = Rc::new(current);
    crate::plugins::switch::follow(ID, move |on| switched(on, current().filter(|rpc| !rpc.is_closed()).as_ref()));
    engine::get().on_connect(connected);
}

/// A connection: the streams opened on it, if the plugin is on.
pub(crate) fn connected(rpc: &Rpc) {
    if on() {
        open(rpc);
    }
}

/// The plugin turned on (each window given its values, the streams opened on
/// `rpc`, the connection there is) or off (the streams closed and the
/// values nil).
fn switched(on: bool, rpc: Option<&Rpc>) {
    if on {
        orchestration::show();
        orchestration_timeline::show();
        orchestration_progress::show();
        scratchpad::show();
        if let Some(rpc) = rpc {
            open(rpc);
        }
    } else {
        orchestration::close();
        orchestration_timeline::close();
        orchestration_progress::close();
        scratchpad::close();
    }
}

/// Every stream of the plugin's on `rpc` (the timeline has none: it reads
/// `sessions`).
fn open(rpc: &Rpc) {
    orchestration::open(rpc);
    orchestration_progress::open(rpc);
    scratchpad::open(rpc);
}

/// A window's shared value of the plugin's, made the first time it's given
/// one: a window that has never had the plugin on has none, and the scripts'
/// `ui.shared` default stands, as without the plugin.
#[derive(Clone)]
pub(crate) struct Key {
    host: ScriptHost,
    name: &'static str,
    made: Rc<RefCell<Option<Shared>>>,
}

impl Key {
    pub(crate) fn new(host: &ScriptHost, name: &'static str) -> Self {
        Self { host: host.clone(), name, made: Rc::default() }
    }

    pub(crate) fn set(&self, value: Value) {
        let shared = self.made.borrow_mut().get_or_insert_with(|| self.host.shared(self.name, Value::Nil)).clone();
        shared.set(value);
    }

    /// Nil, if it was ever given anything.
    pub(crate) fn clear(&self) {
        let made = self.made.borrow().clone();
        if let Some(shared) = made {
            shared.set(Value::Nil);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::Value as Json;
    use std::collections::BTreeMap;
    use zgx_script::testing::TestApp;

    const KEYS: &[&str] = &[
        "orchestration",
        "orchestration.loads",
        "orchestration.comms",
        "orchestration.progress",
        "orchestration.timeline",
        "scratchpad",
    ];

    fn window() -> TestApp {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/activity_fixture.luau");
        TestApp::new(|runtime| ScriptHost::new(runtime, path))
    }

    fn keys(app: &TestApp) -> Vec<(&'static str, bool)> {
        KEYS.iter().map(|k| (*k, app.host.shared(k, Value::Nil).get() != Value::Nil)).collect()
    }

    /// The board's cards as the scripts read them (`orchestration.cards`):
    /// their ids, or nil.
    fn cards(app: &TestApp) -> Option<Vec<String>> {
        let now = app.host.shared("orchestration", Value::Nil).get();
        let cards = now.field("cards")?.clone();
        Some(cards.list().iter().filter_map(|c| match c.field("id") {
            Some(Value::Str(id)) => Some(id.to_string()),
            _ => None,
        }).collect())
    }

    /// A `WatchOrchestration` snapshot with one card on the board.
    fn with_a_card() -> Json {
        let at = "2026-11-22T09:00:00Z";
        serde_json::json!({
            "orchestrators": [], "crew": [], "items": [], "asks": [], "dumps": [],
            "cards": [{ "id": "it-1", "place": "running", "status": "working", "since": at,
                        "statusSince": at, "seq": 1, "updatedAt": at }],
        })
    }

    /// Each stream the frames sent so far opened, by id, and whether it was
    /// cancelled since.
    fn streams(sent: &std::sync::mpsc::Receiver<String>, seen: &mut BTreeMap<u64, (String, bool)>) -> Vec<(String, bool)> {
        while let Ok(frame) = sent.try_recv() {
            let frame: Json = serde_json::from_str(&frame).unwrap();
            let id = frame["id"].as_u64().unwrap();
            if let Some(method) = frame["method"].as_str() {
                seen.insert(id, (method.to_string(), false));
            } else if frame["cancel"] == true {
                seen.get_mut(&id).expect("a cancel of a stream opened").1 = true;
            }
        }
        seen.values().cloned().collect()
    }

    /// The streams open now, by method, sorted.
    fn open_now(sent: &std::sync::mpsc::Receiver<String>, seen: &mut BTreeMap<u64, (String, bool)>) -> Vec<String> {
        let mut open: Vec<String> = streams(sent, seen).into_iter().filter(|(_, cancelled)| !cancelled).map(|(m, _)| m).collect();
        open.sort();
        open
    }

    const STREAMS: [&str; 4] = ["WatchComms", "WatchOrchestration", "WatchProgress", "WatchScratchpad"];

    /// Off (a fresh install's default): a window's connection opens no
    /// orchestration stream, and the scripts are given none of its values.
    /// Turned on: each value set and the streams opened, at once, on the
    /// connection there is. Off again: every stream closed and each value
    /// nil. On again: opened again.
    #[test]
    fn the_streams_and_values_follow_the_switch() {
        let app = window();
        let (rpc, sent, _bell) = crate::engine::rpc::test_connection();
        let there = rpc.clone();
        follow(move || Some(there.clone()));
        add(&app.host);
        connected(&rpc);
        let mut seen = BTreeMap::new();
        assert_eq!(streams(&sent, &mut seen), vec![], "off: nothing opened");
        assert!(keys(&app).iter().all(|(_, set)| !set), "off: no value given: {:?}", keys(&app));
        assert_eq!(cards(&app), None, "off: no cards");

        crate::plugins::switch::set(["git", ID]);
        assert_eq!(open_now(&sent, &mut seen), STREAMS, "on: opened at once");
        assert!(keys(&app).iter().all(|(_, set)| *set), "on: every value given: {:?}", keys(&app));
        // The board's cards, as the stream says them.
        crate::orchestration::heard_for_tests(with_a_card());
        assert_eq!(cards(&app), Some(vec!["it-1".to_string()]), "on: the cards");
        // A connection after: opened on it, the old ones closed.
        connected(&rpc);
        assert_eq!(open_now(&sent, &mut seen), STREAMS);

        crate::plugins::switch::set(["git"]);
        assert_eq!(open_now(&sent, &mut seen), Vec::<String>::new(), "off: every stream closed");
        assert!(keys(&app).iter().all(|(_, set)| !set), "off: every value nil: {:?}", keys(&app));
        assert_eq!(cards(&app), None, "off: the cards gone with it");
        let opened = seen.len();
        connected(&rpc);
        assert_eq!(open_now(&sent, &mut seen), Vec::<String>::new());
        assert_eq!(seen.len(), opened, "off: a connection opens nothing");

        crate::plugins::switch::set([ID]);
        assert_eq!(open_now(&sent, &mut seen), STREAMS, "on again: opened again");
        assert!(keys(&app).iter().all(|(_, set)| *set));
    }

    /// Off, the plugin's actions answer "off" (before anything is sent).
    #[test]
    fn the_actions_are_refused_while_off() {
        assert_eq!(refuse(), Err("off".to_string()));
        crate::plugins::switch::set([ID]);
        assert_eq!(refuse(), Ok(()));
    }

    /// The board's own actions, as a window has them: off, the person's ops
    /// on a card (done, dropped, kept, seen) and a delete waiting for its
    /// Undo answer "off", nothing waits, and the quit sends nothing. On, a
    /// delete waits (and the quit would send it); turned off meanwhile, it's
    /// let go of: never sent, at its wait's end or at the quit.
    #[test]
    fn the_board_s_ops_and_its_delayed_delete_follow_the_switch() {
        let script = r#"
local out = ui.state("out", "")
local function calls(id: string, action: string, arg: any)
    ui.command { id = id, title = id, run = function()
        out:set("")
        ui.call(action, arg):after(function(v) out:set(`ok {v}`) end, function(e) out:set(`rejected {e}`) end)
    end }
end
calls("close", "orchestration.op", { op = "closeItem", id = "it-1", dropped = true })
calls("reopen", "orchestration.op", { op = "reopenItem", id = "it-1" })
calls("hold", "orchestration.op", { op = "holdItem", id = "it-1", held = true })
calls("seen", "orchestration.op", { op = "seenItem", id = "it-1" })
calls("remove", "orchestration.remove_later", { id = "it-1", ms = 60000 })
ui.command { id = "undo", title = "undo", run = function() ui.invoke("orchestration.remove_undo", "it-1") end }
return ui.column { ui.text { id = "out", function() return out:get() end } }
"#;
        let mut app = TestApp::new(move |runtime| {
            let host = ScriptHost::inline(runtime, "app", script);
            let window = orchestration::add(&host);
            orchestration::actions(&host, window);
            host
        });
        // (Followed with no connection: the switch's close reaches the deletes.)
        follow(|| None);
        let said = |app: &mut TestApp, command: &str| {
            assert!(app.host.run_command(command), "{command}");
            assert!(app.wait_until(|app| !app.text("out").is_empty()), "{command}: no answer: {:?}", app.error());
            app.text("out")
        };
        for op in ["close", "reopen", "hold", "seen", "remove"] {
            let out = said(&mut app, op);
            assert!(out.starts_with("rejected") && out.contains("off"), "{op} while off: {out}");
        }
        assert!(app.host.run_command("undo"));
        assert_eq!(orchestration::waiting_deletes(), Vec::<String>::new(), "off: no delete waits");
        assert_eq!(orchestration::sent_at_quit(), Vec::<String>::new(), "off: the quit sends nothing");

        // On: a delete waits for its Undo, and the quit would send it.
        crate::plugins::switch::set([ID]);
        assert!(app.host.run_command("remove"));
        assert!(
            app.wait_until(|_| orchestration::waiting_deletes() == ["it-1"]),
            "on: the delete waits: {:?} {}",
            app.error(),
            app.text("out")
        );
        assert!(!app.text("out").contains("off"), "on: not refused: {}", app.text("out"));
        assert_eq!(orchestration::sent_at_quit(), vec!["it-1".to_string()], "on: the quit sends it");

        // Waiting again, and turned off: let go of, the quit sends nothing.
        assert!(app.host.run_command("remove"));
        assert!(app.wait_until(|_| orchestration::waiting_deletes() == ["it-1"]));
        crate::plugins::switch::set([]);
        assert_eq!(orchestration::waiting_deletes(), Vec::<String>::new(), "off: let go of");
        assert_eq!(orchestration::sent_at_quit(), Vec::<String>::new(), "off: the quit sends nothing");
    }
}
