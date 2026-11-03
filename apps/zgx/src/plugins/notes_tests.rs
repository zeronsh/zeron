//! Zeron's Notes plugin (plugins-builtin/notes) on the real panel home
//! (ui/panel_home_fixture.luau), headless: loaded as the bridge loads a
//! snapshot, granted what its manifest asks (or not "notes"), with the
//! host's way to the notes (src/plugins/notes.rs) made as the bridge makes
//! it, on an engine played here that compares and sets on the text's hash as
//! crates/doc does.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::Duration,
};

use serde_json::{Value as Json, json};
use zgx::input::InputEvent;
use zgx_script::{ScriptHost, Value, testing::TestApp};

use super::notes::{self, Engine, Notes, Reply};
use super::{Item, expose_own, expose_surface, json_value, value_json};

/// crates/doc `notes_hash`: FNV-1a 64, 16 lowercase hex digits.
fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

type Listener = Box<dyn FnMut(Json)>;

/// The engine's notes, played: one text for every chat a test touches
/// (`chat-1`'s), watches told of each change, writes compare-and-set on
/// the hash. `old`: an engine from before notes.
#[derive(Default)]
struct Played {
    text: RefCell<String>,
    seq: Cell<u64>,
    old: Cell<bool>,
    next: Cell<u64>,
    listeners: RefCell<Vec<(u64, String, Listener)>>,
    /// What was asked: `watch:<chat>`, `set:<chat>:<text>`.
    asked: RefCell<Vec<String>>,
    me: RefCell<Weak<Played>>,
}

struct Handle {
    id: u64,
    engine: Weak<Played>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(e) = self.engine.upgrade()
            && let Ok(mut all) = e.listeners.try_borrow_mut()
        {
            all.retain(|(id, _, _)| *id != self.id);
        }
    }
}

impl Played {
    fn new() -> Rc<Played> {
        let p = Rc::new(Played::default());
        *p.me.borrow_mut() = Rc::downgrade(&p);
        p
    }

    fn snapshot(&self) -> Json {
        let text = self.text.borrow().clone();
        json!({ "text": text, "hash": hash(&text), "epoch": 7, "seq": self.seq.get() })
    }

    /// Every watch told what the notes are now (with no borrow held: what
    /// it tells may watch again).
    fn publish(&self) {
        self.seq.set(self.seq.get() + 1);
        let snapshot = self.snapshot();
        let mut all = std::mem::take(&mut *self.listeners.borrow_mut());
        for (_, _, listener) in all.iter_mut() {
            listener(snapshot.clone());
        }
        let mut now = self.listeners.borrow_mut();
        all.append(&mut now);
        *now = all;
    }

    /// Another computer writes `line` at the notes' top.
    fn remote(&self, line: &str) {
        let now = format!("{line}{}", self.text.borrow());
        *self.text.borrow_mut() = now;
        self.publish();
    }

    fn asked(&self, prefix: &str) -> Vec<String> {
        self.asked.borrow().iter().filter(|a| a.starts_with(prefix)).cloned().collect()
    }
}

impl Engine for Played {
    fn watch(&self, chat: &str, mut on_item: Box<dyn FnMut(Json)>, on_end: Box<dyn FnOnce(String)>) -> Option<Box<dyn Any>> {
        self.asked.borrow_mut().push(format!("watch:{chat}"));
        if self.old.get() {
            on_end("unknown method: WatchSessionNotes".into());
            return Some(Box::new(()));
        }
        let id = self.next.get() + 1;
        self.next.set(id);
        on_item(self.snapshot());
        self.listeners.borrow_mut().push((id, chat.to_string(), on_item));
        Some(Box::new(Handle { id, engine: self.me.borrow().clone() }))
    }

    fn set(&self, params: Json) -> Reply {
        let me = self.me.borrow().clone();
        Box::pin(async move {
            let e = me.upgrade().ok_or("gone")?;
            let (chat, base, text) = (params["chatId"].as_str().unwrap_or_default(), params["baseHash"].as_str().unwrap_or_default(), params["text"].as_str().unwrap_or_default());
            e.asked.borrow_mut().push(format!("set:{chat}:{text}"));
            if e.old.get() {
                return Err("unknown method: SetSessionNotes".to_string());
            }
            if base != hash(&e.text.borrow()) {
                return Ok(json!({ "kind": "conflict", "current": e.snapshot() }));
            }
            *e.text.borrow_mut() = text.to_string();
            e.publish();
            let s = e.snapshot();
            Ok(json!({ "kind": "applied", "hash": s["hash"], "epoch": s["epoch"], "seq": s["seq"] }))
        })
    }

    fn later(&self, _wait: Duration, _work: Box<dyn FnOnce()>) {}
}

struct World {
    app: TestApp,
    engine: Rc<Played>,
    notes: Rc<Notes>,
}

/// The Notes plugin as Zeron ships it (plugins-builtin/notes), allowed
/// what its manifest asks; `without` leaves those out of what it was
/// allowed.
fn plugin(without: &[&str]) -> Item {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins-builtin/notes");
    let manifest: Json = serde_json::from_str(&std::fs::read_to_string(dir.join("zeron-plugin.json")).unwrap()).unwrap();
    let permissions: Vec<Json> = manifest["permissions"].as_array().unwrap().iter().filter(|p| !without.contains(&p.as_str().unwrap())).cloned().collect();
    Item {
        json: json!({
            "id": "notes", "name": manifest["name"], "status": "on", "entry": manifest["entry"],
            "snapshot": dir, "permissions": permissions, "ui": {},
        }),
    }
}

/// The real panel home with the Notes plugin (`item`), chat-1's page.
fn world(item: Item) -> World {
    let entry: PathBuf = crate::ui_dir().join("panel_home_fixture.luau");
    let engine = Played::new();
    let allowed = item.clone();
    let app = TestApp::new(move |runtime| {
        let host = super::lend_design_system(ScriptHost::new(runtime, &entry), &entry);
        host.shared("zgx.window.size", zgx_script::window_size_value(1000., 900.));
        expose_surface(&host);
        let id = item.id().to_string();
        expose_own(&host, &id);
        super::locales::publish(&host, &id, Path::new(item.json["snapshot"].as_str().unwrap()));
        for name in ["result", "ack", "menus", "toast", "problem", "add_page", "cost", "github_watch", "open_url"] {
            host.action(format!("plugin.{id}.{name}"), |_| {});
        }
        host.shared(&format!("plugin.{id}.records"), json_value(&json!({ "profile": {} })));
        // Its storage, as the host keeps it: written, then published.
        let store = host.shared(&format!("plugin.{id}.store"), json_value(&json!({})));
        host.action(format!("plugin.{id}.store_set"), move |v: Value| {
            let asked = value_json(&v);
            let mut now = value_json(&store.signal().with_untracked(Value::clone));
            if !now.is_object() {
                now = json!({});
            }
            let key = asked["key"].as_str().unwrap().to_string();
            if asked["value"].is_null() {
                now.as_object_mut().unwrap().remove(&key);
            } else {
                now[key] = asked["value"].clone();
            }
            store.set(json_value(&now));
        });
        // Its tabs, as the bridge checks them (surfaces) and the app opens
        // them (the fixture, from `test_plugin_tabs`).
        let tabs = host.shared("test_plugin_tabs", json_value(&json!([])));
        let plugin = id.clone();
        host.action(format!("plugin.{id}.open_tab"), move |v: Value| {
            let Ok(mut tab) = super::surfaces::tab(&value_json(&v)) else { return };
            tab["plugin"] = json!(plugin);
            let mut all = value_json(&tabs.signal().with_untracked(Value::clone));
            if !all.is_array() {
                all = json!([]);
            }
            all.as_array_mut().unwrap().push(tab);
            tabs.set(json_value(&all));
        });
        host.shared("zeron.page_session", Value::from("chat-1"));
        host.set_plugins(item.plugin().into_iter().collect());
        host
    });
    app.ui.scene.borrow_mut().resize(1000., 900.);
    // The way to the notes, as the bridge makes it (once the app's scripts
    // have their sessions, as the bridge's plugins come after them).
    let notes = Notes::new(&app.host, &app.ui.runtime, engine.clone() as Rc<dyn Engine>, move |_| allowed.may(notes::PERMISSION));
    notes.expose(&app.host, "notes");
    let mut w = World { app, engine, notes };
    w.app.frames(20);
    assert_eq!(w.app.error(), None);
    assert!(w.app.wait_until(|app| app.find("#panel-empty").is_some()), "{}", w.app.tree());
    w.app.frames(10);
    w
}

const ROW: &str = "#notes-open";
const EDITOR: &str = "#notes-editor";

impl World {
    /// A named state's value (`ui.state`, a module's own, or `ui.shared`).
    fn state(&self, name: &str) -> Json {
        let suffix = format!("::{name}");
        self.app
            .host
            .states()
            .into_iter()
            .find(|(n, _)| n == name || n.ends_with(&suffix))
            .map(|(_, v)| value_json(&v))
            .unwrap_or(Json::Null)
    }

    /// The tabs of `chat`'s panel, by kind (a plugin's: `plugin:<kind>`).
    fn kinds(&self, chat: &str) -> String {
        let all = self.state("panel_sessions");
        all[format!("chat:{chat}")]["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| if t["kind"] == "plugin" { format!("plugin:{}", t["plugin_kind"].as_str().unwrap_or("")) } else { t["kind"].as_str().unwrap_or("").to_string() })
            .collect::<Vec<_>>()
            .join(",")
    }

    fn active(&self, chat: &str) -> String {
        let s = &self.state("panel_sessions")[format!("chat:{chat}")];
        let tab = s["tabs"].as_array().into_iter().flatten().find(|t| t["id"] == s["active"]).cloned().unwrap_or_default();
        tab["plugin_kind"].as_str().or(tab["kind"].as_str()).unwrap_or("").to_string()
    }

    /// What the editor shows (its value, as the tree says it; "" for none).
    fn shown(&mut self) -> String {
        let tree = self.app.tree();
        let line = tree.lines().find(|l| l.contains("#notes-editor ")).unwrap_or_default();
        let Some(at) = line.find(" =\"") else { return String::new() };
        let rest = &line[at + 3..];
        let end = rest.rfind('"').unwrap_or(rest.len());
        rest[..end].replace('⏎', "\n")
    }

    fn engine_text(&self) -> String {
        self.engine.text.borrow().clone()
    }

    /// The page shows `chat` (a side chat's, a subagent's), its panel too.
    fn show(&mut self, chat: &str) {
        self.app.host.shared("zeron.page_session", Value::Nil).set(Value::from(chat));
        self.app.host.set_state("test_home_session", json_value(&json!({ "id": chat, "cwd": "/proj" }))).unwrap();
        self.app.frames(20);
    }

    fn open(&mut self) {
        self.app.click(ROW);
        assert!(self.app.wait_until(|app| app.find(EDITOR).is_some()), "{}", self.app.tree());
        self.app.frames(5);
    }

    fn back_home(&mut self) {
        self.app.click("role:button \"New Tab\"");
        assert!(self.app.wait_until(|app| app.find("#panel-empty").is_some()), "{}", self.app.tree());
        self.app.frames(5);
    }

    /// Text typed where the caret is (no click: it stays where it is).
    fn type_here(&mut self, text: &str) {
        self.app.ui.dispatch(InputEvent::Text(text.into()));
        self.app.settle();
        self.app.frames(2);
    }

    fn wait(&mut self, what: &str, mut ok: impl FnMut(&mut World) -> bool) {
        for _ in 0..600 {
            if ok(self) {
                return;
            }
            self.app.frames(2);
        }
        panic!("never: {what}\n{}", self.app.tree());
    }
}

#[test]
fn a_main_chat_offers_notes_with_n() {
    let mut w = world(plugin(&[]));
    assert!(w.app.wait_until(|app| app.find(ROW).is_some()), "{}", w.app.tree());
    assert!(w.app.find(&format!("{ROW} >> \"Notes\"")).is_some(), "{}", w.app.tree());
    assert!(w.app.find(&format!("{ROW} >> \"N\"")).is_some(), "{}", w.app.tree());
    // Among the home's own rows, after them.
    assert!(w.app.find("#panel-home-open >> #notes-open").is_some(), "{}", w.app.tree());
}

#[test]
fn a_side_chat_s_panel_and_a_subagent_s_have_no_notes_and_n_opens_nothing() {
    let mut w = world(plugin(&[]));
    assert!(w.app.wait_until(|app| app.find(ROW).is_some()), "{}", w.app.tree());
    for chat in ["side-a", "chat-1--sub--1"] {
        w.show(chat);
        assert!(w.app.wait_until(|app| app.find("#panel-empty").is_some()), "{}", w.app.tree());
        assert!(w.app.find(ROW).is_none(), "{chat}: {}", w.app.tree());
        w.app.click("#outside");
        w.app.keys("n");
        w.app.frames(10);
        assert_eq!(w.kinds(chat), "", "{chat}");
        assert!(w.app.find(EDITOR).is_none(), "{chat}");
    }
    // Nothing of theirs was watched or written.
    assert!(w.engine.asked("watch:side").is_empty() && w.engine.asked("watch:chat-1--sub").is_empty(), "{:?}", w.engine.asked.borrow());
    // Back on the main chat, the row is back.
    w.show("chat-1");
    assert!(w.app.wait_until(|app| app.find(ROW).is_some()), "{}", w.app.tree());
}

#[test]
fn notes_open_one_tab_with_a_blank_page_and_opening_again_comes_back_to_it() {
    let mut w = world(plugin(&[]));
    w.open();
    assert_eq!(w.kinds("chat-1"), "plugin:notes");
    assert!(w.app.find("#panel-strip >> role:tab \"Notes\"").is_some(), "{}", w.app.tree());
    assert_eq!(w.shown(), "");
    assert!(w.app.find("\"Notes for this session. Markdown works here.\"").is_some(), "{}", w.app.tree());
    w.back_home();
    w.app.click("#panel-open-terminal");
    w.wait("a terminal beside the notes", |w| w.kinds("chat-1") == "plugin:notes,terminal");
    w.back_home();
    w.open();
    assert_eq!(w.kinds("chat-1"), "plugin:notes,terminal");
    assert_eq!(w.active("chat-1"), "notes");
}

#[test]
fn n_opens_the_notes_from_the_home_with_the_keyboard_in_them() {
    let mut w = world(plugin(&[]));
    assert!(w.app.wait_until(|app| app.find(ROW).is_some()), "{}", w.app.tree());
    w.app.click("#outside");
    w.app.keys("n");
    w.wait("the notes open", |w| w.kinds("chat-1") == "plugin:notes" && w.app.find(EDITOR).is_some());
    w.app.frames(10);
    // The keyboard is in the editor: what's typed lands there.
    w.type_here("typed");
    assert_eq!(w.shown(), "typed");
}

#[test]
fn typing_is_written_after_a_quiet_and_found_again_as_the_tab_comes_back() {
    let mut w = world(plugin(&[]));
    w.open();
    w.app.type_text(EDITOR, "- [ ] ship notes");
    w.wait("the write", |w| w.engine_text() == "- [ ] ship notes");
    // One write, after the typing stopped.
    assert_eq!(w.engine.asked("set:").len(), 1, "{:?}", w.engine.asked.borrow());
    assert_eq!(w.engine.asked("set:chat-1:").len(), 1);
    // Closed and opened again: the same words.
    w.app.keys("cmd+w");
    w.wait("the tab closed", |w| w.kinds("chat-1").is_empty());
    assert!(w.app.wait_until(|app| app.find("#panel-empty").is_some()), "{}", w.app.tree());
    w.open();
    assert_eq!(w.shown(), "- [ ] ship notes");
}

#[test]
fn what_the_engine_holds_shows_as_the_notes_open() {
    let mut w = world(plugin(&[]));
    *w.engine.text.borrow_mut() = "# Plan\n".into();
    w.engine.remote("From the laptop\n");
    w.open();
    w.wait("the engine's text", |w| w.shown() == "From the laptop\n# Plan\n");
}

#[test]
fn another_computer_s_change_lands_in_place_and_the_caret_stays_where_it_was() {
    let mut w = world(plugin(&[]));
    w.open();
    w.app.type_text(EDITOR, "hello world");
    w.wait("the write", |w| w.engine_text() == "hello world");
    w.engine.remote("From the laptop\n");
    w.wait("the merge", |w| w.shown() == "From the laptop\nhello world");
    // Typing goes on where the caret was: after "world", not at the top.
    w.type_here("!");
    assert_eq!(w.shown(), "From the laptop\nhello world!");
    w.wait("the write", |w| w.engine_text() == "From the laptop\nhello world!");
}

#[test]
fn an_edit_racing_another_computer_s_keeps_both() {
    let mut w = world(plugin(&[]));
    w.open();
    w.app.type_text(EDITOR, "one");
    w.wait("the write", |w| w.engine_text() == "one");
    // Typed here, not written yet, as the other computer writes.
    w.type_here(" two");
    w.engine.remote("From the laptop\n");
    w.wait("both kept", |w| w.engine_text() == "From the laptop\none two" && w.shown() == "From the laptop\none two");
}

#[test]
fn an_engine_from_before_notes_keeps_them_here_then_hands_them_over() {
    let mut w = world(plugin(&[]));
    *w.engine.text.borrow_mut() = "synced line".into();
    w.engine.old.set(true);
    w.open();
    w.wait("the local word", |w| w.app.find("#notes-local >> \"Syncs once Zeron updates\"").is_some());
    w.app.type_text(EDITOR, "kept here");
    w.app.frames(5);
    // Kept in the plugin's own store (removed with it), never written.
    assert_eq!(w.state("plugin.notes.store")["local"]["chat-1"], "kept here");
    assert!(w.engine.asked("set:").is_empty(), "{:?}", w.engine.asked.borrow());
    // The engine updated, and back: the kept words go into the synced ones.
    w.engine.old.set(false);
    w.notes.reconnect();
    w.wait("handed over", |w| w.engine_text() == "synced line\nkept here");
    assert_eq!(w.shown(), "synced line\nkept here");
    w.app.frames(5);
    assert!(w.app.find("#notes-local").is_none(), "{}", w.app.tree());
    assert!(w.state("plugin.notes.store")["local"].get("chat-1").is_none(), "{}", w.state("plugin.notes.store"));
}

#[test]
fn the_merge_keeps_both_sides_never_splitting_a_character_and_the_hash_is_the_engine_s() {
    // (merge.luau, run as the plugin runs it.)
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins-builtin/notes/merge.luau")).unwrap().replace("export type", "type");
    let code = format!(
        "local Doc = (function()\n{src}\nend)()
        local e = Doc.diff('é', 'è') :: any
        local said = table.concat({{
            Doc.hash(''), Doc.hash('a'), Doc.hash('# Plan\\n- [ ] é ship notes 🚀'),
            Doc.rebase('abc', 'aXbc', 'abcY'), Doc.rebase('abc', 'abc', 'abcY'),
            Doc.rebase('hello', 'help', 'hello!'), Doc.rebase('', 'mine', 'theirs'),
            `{{e.from}},{{e.to}},{{e.insert}}`,
        }}, '|')
        return ui.text {{ id = 'said', said, literal = true }}"
    );
    let mut app = TestApp::inline(&code);
    assert_eq!(app.error(), None);
    let said = app.text("said");
    let parts: Vec<&str> = said.split('|').collect();
    // The hash is crates/doc `notes_hash`'s, the engine's own values.
    assert_eq!(parts[..3], ["cbf29ce484222325", "af63dc4c8601ec8c", "6d31b6c8ea457310"], "{said}");
    assert_eq!(hash("# Plan\n- [ ] é ship notes 🚀"), "6d31b6c8ea457310");
    // Both sides kept; overlapping deletions honoured; never half a character.
    assert_eq!(parts[3..], ["aXbcY", "abcY", "help!", "theirsmine", "0,2,è"], "{said}");
}

// -- the grant ---------------------------------------------------------------

#[test]
fn a_plugin_without_notes_hears_none_and_writes_none() {
    let mut w = world(plugin(&["notes"]));
    w.app.frames(20);
    // No row: Zeron names no chat to it.
    assert!(w.app.find(ROW).is_none(), "{}", w.app.tree());
    assert!(nothing(&w.state("plugin.notes.notes")), "{}", w.state("plugin.notes.notes"));
    // Asking anyway: nothing watched, nothing told, and a write refused.
    w.notes.want("notes", &json!(["chat-1"]));
    let refused = w.notes.set("notes", &json_value(&json!({ "chatId": "chat-1", "baseHash": hash(""), "text": "x" })));
    assert!(refused.as_ref().is_err_and(|e| e.starts_with("not_allowed")), "{:?}", refused.err());
    assert!(w.engine.asked.borrow().is_empty(), "{:?}", w.engine.asked.borrow());
    assert!(nothing(&w.state("plugin.notes.notes")), "{}", w.state("plugin.notes.notes"));
}

#[test]
fn notes_reach_only_the_chat_the_plugin_is_shown_in() {
    let mut w = world(plugin(&["ui.add", "panel.tabs"]));
    w.app.frames(10);
    let set = |w: &World, chat: &str| w.notes.set("notes", &json_value(&json!({ "chatId": chat, "baseHash": hash(""), "text": "x" }))).err();
    // Before it asks for chat-1 (its page), not even chat-1's may be written.
    assert!(set(&w, "chat-1").is_some_and(|e| e.starts_with("not_allowed")));
    // Zeron names the page's main chat, and only that one.
    assert_eq!(w.state("plugin.notes.notes")["chat"], "chat-1");
    // Asked for chat-1 and another main chat (chat-2) and a side chat:
    // only chat-1 is watched and told.
    w.notes.want("notes", &json!(["chat-1", "chat-2", "side-a"]));
    assert_eq!(w.engine.asked("watch:"), vec!["watch:chat-1".to_string()]);
    let told = w.state("plugin.notes.notes");
    assert!(told["notes"].get("chat-1").is_some() && told["notes"].get("chat-2").is_none() && told["notes"].get("side-a").is_none(), "{told}");
    assert!(set(&w, "chat-1").is_none());
    assert!(set(&w, "chat-2").is_some_and(|e| e.starts_with("not_allowed")));
    assert!(set(&w, "side-a").is_some_and(|e| e.starts_with("not_allowed")));
    // The page moves to a side chat: no chat is named, chat-1 stays open
    // (its unwritten words still land), and the side chat isn't opened.
    w.show("side-a");
    assert!(w.state("plugin.notes.notes").get("chat").is_none(), "{}", w.state("plugin.notes.notes"));
    assert!(set(&w, "chat-1").is_none());
    assert!(set(&w, "side-a").is_some_and(|e| e.starts_with("not_allowed")));
    assert_eq!(w.engine.asked("watch:"), vec!["watch:chat-1".to_string()]);
}

/// An empty table (said as `{}` or `[]`).
fn nothing(v: &Json) -> bool {
    v.as_object().is_some_and(|o| o.is_empty()) || v.as_array().is_some_and(|a| a.is_empty())
}
