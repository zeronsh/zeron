//! A settings page a plugin draws itself (`zeron.settings_page`) in Zeron's
//! real Settings, headless: ui/plugin_settings_page_fixture.luau (the shell
//! with screens/plugins) with real plugins loaded from their folders
//! (plugins-dev/settings-page, and small ones written here), each granted
//! what its manifest asks, and the bridge's part played here with the real
//! settings book (settings.rs): what a plugin's SDK says of its page, what
//! its page and Zeron's rows set, each published back as the bridge does.

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
};

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Shared, Value, testing::TestApp};

use super::{
    Item, expose_own, expose_surface, json_value,
    settings::{Book, On, manifest_in},
    value_json,
};

const NAV: &str = "#settings-nav";
const PAGE: &str = "#settings-page";
const OWN: &str = "#plugin-settings-page";

/// What a plugin said of its page (`settings_page`), taken after the frame
/// it was said in, as the bridge takes it after the turn.
type Said = Rc<RefCell<VecDeque<(String, Json)>>>;
/// Every other of its own actions, as asked: (`plugin.<id>.<name>`, value).
type Heard = Rc<RefCell<Vec<(String, Json)>>>;

/// A plugin folder written here: its manifest, `init.luau` and the SDK.
fn written(name: &str, manifest: Json, init: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-settings-page-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("zeron-plugin.json"), serde_json::to_string(&manifest).unwrap()).unwrap();
    std::fs::write(dir.join("init.luau"), init).unwrap();
    std::fs::write(dir.join("zeron.luau"), include_str!("../../plugins-sdk/zeron.luau")).unwrap();
    dir
}

fn example(id: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins-dev").join(id)
}

fn item(dir: &Path) -> Item {
    let manifest = manifest_in(dir).expect("a manifest");
    Item {
        json: json!({
            "id": manifest["id"], "name": manifest["name"], "status": "on", "entry": manifest["entry"],
            "snapshot": dir, "permissions": manifest["permissions"],
            "ui": manifest.get("ui").cloned().unwrap_or(json!({})),
        }),
    }
}

struct Rig {
    app: TestApp,
    book: Rc<RefCell<Book>>,
    said: Said,
    heard: Heard,
    items: Vec<Item>,
}

/// The bridge's publish: Settings' list, and each plugin's own values (set
/// with no borrow held: what they move runs at once, and may set again).
fn publish(book: &RefCell<Book>, list: &Shared, own: &[(String, Shared)]) {
    let (now, values): (Json, Vec<Json>) = {
        let book = book.borrow();
        (book.list(), own.iter().map(|(id, _)| Json::Object(book.values(id))).collect())
    };
    list.set(json_value(&now));
    for ((_, shared), value) in own.iter().zip(values) {
        shared.set(json_value(&value));
    }
}

impl Rig {
    fn new(dirs: &[PathBuf]) -> Rig {
        let items: Vec<Item> = dirs.iter().map(|d| item(d)).collect();
        let book = Rc::new(RefCell::new(Book::new(None)));
        let on: Vec<On> = items.iter().map(|i| On { id: i.id().into(), name: i.name().into(), hash: "h1".into(), builtin: false }).collect();
        book.borrow_mut().sync(&on, |id| items.iter().find(|i| i.id() == id).and_then(|i| manifest_in(Path::new(i.json["snapshot"].as_str().unwrap()))));
        let (said, heard) = (Said::default(), Heard::default());
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/plugin_settings_page_fixture.luau");
        let (b, s, h, its) = (book.clone(), said.clone(), heard.clone(), items.clone());
        let app = TestApp::new(move |runtime| {
            let host = super::lend_design_system(ScriptHost::new(runtime, path.clone()), &path);
            host.shared("zgx.window.size", zgx_script::window_size_value(1200., 1400.));
            host.action("zeron.open", |_| {});
            host.action("close", |_| {});
            expose_surface(&host);
            let list = host.shared("plugins.settings", json_value(&json!([])));
            let own: Vec<(String, Shared)> = its.iter().map(|i| (i.id().to_string(), host.shared(&format!("plugin.{}.settings", i.id()), json_value(&json!({}))))).collect();
            let own = Rc::new(own);
            let mut plugins = Vec::new();
            for it in &its {
                let id = it.id().to_string();
                expose_own(&host, &id);
                super::locales::publish(&host, &id, Path::new(it.json["snapshot"].as_str().unwrap()));
                for name in ["result", "ack", "menus", "toast", "problem", "cost", "open_tab", "add_page", "setting_status", "open_url", "github_watch"] {
                    let (heard, key) = (h.clone(), format!("plugin.{id}.{name}"));
                    host.action(key.clone(), move |v: Value| heard.borrow_mut().push((key.clone(), value_json(&v))));
                }
                {
                    let (said, id) = (s.clone(), id.clone());
                    host.action(format!("plugin.{id}.settings_page"), move |v: Value| said.borrow_mut().push_back((id.clone(), value_json(&v))));
                }
                {
                    // (Its storage as the host keeps it: written, then published.)
                    let store = host.shared(&format!("plugin.{id}.store"), json_value(&json!({})));
                    host.action(format!("plugin.{id}.store_set"), move |v: Value| {
                        let asked = value_json(&v);
                        let mut now = value_json(&store.signal().with_untracked(Value::clone));
                        if !now.is_object() {
                            now = json!({});
                        }
                        now[asked["key"].as_str().unwrap()] = asked["value"].clone();
                        store.set(json_value(&now));
                    });
                }
                {
                    // Its own settings, from its page: the book's, as the bridge's.
                    let (book, list, own, id, heard) = (b.clone(), list.clone(), own.clone(), id.clone(), h.clone());
                    host.async_action(format!("plugin.{id}.setting_set"), move |v: Value| {
                        let asked = value_json(&v);
                        heard.borrow_mut().push((format!("plugin.{id}.setting_set"), asked.clone()));
                        let done = book.borrow_mut().set_own(&id, asked["id"].as_str().unwrap_or_default(), &asked["value"]);
                        let answer = done.map(|changed| {
                            if changed {
                                publish(&book, &list, &own);
                            }
                            let now = book.borrow().values(&id).get(asked["id"].as_str().unwrap_or_default()).cloned().unwrap_or(Json::Null);
                            json_value(&now)
                        });
                        async move { answer }
                    });
                }
                {
                    // The person's folder, as the dialog would choose it.
                    let (book, list, own, id) = (b.clone(), list.clone(), own.clone(), id.clone());
                    host.async_action(format!("plugin.{id}.setting_folder"), move |v: Value| {
                        let asked = value_json(&v);
                        let setting = asked["id"].as_str().unwrap_or_default().to_string();
                        let label = book.borrow().folder_label(&id, &setting);
                        let done = label.and_then(|_| book.borrow_mut().set(&id, &setting, &json!("/Users/me/Picked")));
                        if done.is_ok() {
                            publish(&book, &list, &own);
                        }
                        async move { done.map(|_| Value::from("/Users/me/Picked")) }
                    });
                }
                plugins.extend(it.plugin());
            }
            {
                // What Zeron's own rows set (the app's action).
                let (book, list, own) = (b.clone(), list.clone(), own.clone());
                host.action("plugins.setting_set", move |v: Value| {
                    let asked = value_json(&v);
                    let done = book.borrow_mut().set(asked["plugin"].as_str().unwrap_or_default(), asked["id"].as_str().unwrap_or_default(), &asked["value"]);
                    done.unwrap();
                    publish(&book, &list, &own);
                });
            }
            host.set_plugins(plugins);
            host
        });
        app.ui.scene.borrow_mut().resize(1200., 1400.);
        let rig = Rig { app, book, said, heard, items };
        // Settings → Plugins' list (who made each), and Settings' own list.
        let rows: Vec<Json> = rig.items.iter().map(|i| json!({ "id": i.id(), "name": i.name(), "status": "on", "author": { "kind": "you" }, "own": true, "permissions": i.permissions(), "allowed": i.permissions(), "ask": [] })).collect();
        rig.app.host.shared("plugins", Value::Nil).set(json_value(&json!({ "state": "ready", "list": rows })));
        rig.publish();
        rig
    }

    fn publish(&self) {
        let own: Vec<(String, Shared)> = self.items.iter().map(|i| (i.id().to_string(), self.app.host.shared(&format!("plugin.{}.settings", i.id()), Value::Nil))).collect();
        publish(&self.book, &self.app.host.shared("plugins.settings", Value::Nil), &own);
    }

    /// `n` frames, what plugins said of their pages taken after each.
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.app.frames(1);
            let said: Vec<(String, Json)> = self.said.borrow_mut().drain(..).collect();
            let mut changed = false;
            for (plugin, asked) in said {
                let mut book = self.book.borrow_mut();
                changed |= match asked["op"].as_str() {
                    Some("declare") => book.declare_page(&plugin, &asked["keywords"]),
                    Some("built") => book.page_built(&plugin, asked["error"].as_str()),
                    _ => false,
                };
            }
            if changed {
                self.publish();
            }
        }
    }

    /// Settings open on `name`'s page.
    fn open(&mut self, name: &str) {
        self.frames(10);
        assert!(self.app.host.run_command("settings_stub.sign_in"));
        assert!(self.app.host.run_command("shell.settings"));
        self.frames(20);
        self.app.scroll(NAV, 400.);
        self.frames(5);
        self.app.click(&format!("{NAV} >> role:tab \"{name}\""));
        self.frames(30);
    }

    fn value(&self, plugin: &str, id: &str) -> Json {
        self.book.borrow().values(plugin).get(id).cloned().unwrap_or(Json::Null)
    }

    /// The window's whole tree (`tree` keeps to its first 200 lines).
    fn tree(&mut self) -> String {
        self.app.frames(1);
        let options = zgx_script::driver::TreeOptions { budget: usize::MAX, ..Default::default() };
        let (lines, hidden) = zgx_script::driver::tree_lines(&self.app.ui, &self.app.handle, None, &options);
        zgx_script::driver::tree_lines_text(&lines, hidden)
    }

    /// The tree's line for `part` (a control's, with its state).
    fn line(&mut self, part: &str) -> String {
        self.tree().lines().find(|l| l.contains(part)).unwrap_or("").to_string()
    }
}

/// The snippets kept, as the plugin's storage holds them.
fn kept(rig: &Rig) -> Json {
    value_json(&rig.app.host.shared("plugin.settings-page.store", Value::Nil).signal().with_untracked(Value::clone))["snippets"].clone()
}

#[test]
fn snippets_draws_its_own_page_after_its_declared_rows_and_fills_its_room() {
    let mut rig = Rig::new(&[example("settings-page")]);
    assert_eq!(rig.app.error(), None, "the shell with Snippets failed");
    rig.open("Snippets");
    assert_eq!(rig.app.error(), None);
    // Zeron's header and its declared rows, then the plugin's own page.
    assert!(rig.app.find(&format!("{PAGE} >> role:heading \"Snippets\"")).is_some(), "{}", rig.tree());
    assert!(rig.app.find("#setting-insert").is_some(), "its declared rows: {}", rig.tree());
    let list = rig.app.find("#snippets-list");
    assert!(list.is_some(), "its own page drew: {}", rig.tree());
    let (row, own, drawn) = (rig.app.bounds("#setting-insert"), rig.app.bounds(OWN), rig.app.bounds("#snippets-list"));
    assert!(drawn.y >= row.y + row.height, "after the declared rows: {row:?} then {drawn:?}");
    assert!(drawn.height > 100., "its rows take their room: {drawn:?}");
    // As wide as Zeron's own cards, and its box as tall as what it drew.
    let card = rig.app.bounds(&format!("{PAGE} >> role:group \"Message box\""));
    assert!((drawn.width - card.width).abs() < 1. && (drawn.x - card.x).abs() < 1., "{card:?} {drawn:?}");
    assert!(own.height >= drawn.height, "its box holds it: {own:?} {drawn:?}");
    // Its sections apart at the page's rhythm, as Zeron's are ($space.6).
    let (first, editor) = (rig.app.bounds(&format!("{PAGE} >> role:group \"Snippets\"")), rig.app.bounds("#snippets-editor"));
    assert!((editor.y - (first.y + first.height) - 32.).abs() < 0.5, "{first:?} then {editor:?}");
    // And the first a section's gap under Zeron's last card.
    let declared = rig.app.bounds(&format!("{PAGE} >> role:group \"Message box\""));
    assert!((first.y - (declared.y + declared.height) - 32.).abs() < 0.5, "{declared:?} then {first:?}");
    // It said how it built: well, found by its words.
    assert_eq!(rig.book.borrow().page_problem("settings-page"), Some(None));
    assert_eq!(rig.book.borrow().list()[0]["page"]["keywords"], json!(["snippets", "templates", "text", "message box"]));
    assert!(!rig.tree().contains("couldn't be shown"));
}

#[test]
fn a_custom_control_and_its_declared_row_are_one_value() {
    let mut rig = Rig::new(&[example("settings-page")]);
    rig.open("Snippets");
    // The declared row turned off (Zeron's row): the page says the button is
    // hidden, and offers it back.
    rig.app.click(&format!("{PAGE} >> role:switch \"Show the Snippets button\""));
    rig.frames(10);
    assert_eq!(rig.value("settings-page", "show"), json!(false));
    assert!(rig.tree().contains("The Snippets button is hidden"), "{}", rig.tree());
    // Its own button sets the declared value, checked and kept by Zeron.
    rig.app.click("role:button \"Show it\"");
    rig.frames(10);
    assert_eq!(rig.value("settings-page", "show"), json!(true));
    assert!(!rig.tree().contains("The Snippets button is hidden"));
    assert!(rig.line("switch \"Show the Snippets button\"").contains("checked"));
    assert!(!rig.line("switch \"Show the Snippets button\"").contains("unchecked"));
    assert_eq!(rig.app.error(), None);
}

#[test]
fn snippets_edits_its_list_with_master_detail_validation_and_a_result() {
    let mut rig = Rig::new(&[example("settings-page")]);
    rig.open("Snippets");
    // The starters, the first open under the list.
    assert!(rig.app.find("#snippets-list >> \"Ask for a review\"").is_some(), "{}", rig.tree());
    assert!(rig.app.find("#snippets-list >> \"Standup\"").is_some());
    let review_row = rig.app.find("#snippets-list >> role:button \"Ask for a review\"");
    assert!(rig.line("textbox \"Name\"").contains("Ask for a review"), "{}", rig.tree());
    // A text snippet has no Try it.
    assert!(rig.app.find("role:button \"Try it\"").is_none());
    // Another chosen: open under the list; the list's rows stay as they were.
    rig.app.click("#snippets-list >> role:button \"Standup\"");
    rig.frames(20);
    assert!(rig.line("textbox \"Name\"").contains("Standup"));
    assert_eq!(rig.app.find("#snippets-list >> role:button \"Ask for a review\""), review_row, "the list's rows, in place");
    // A template tries itself: what it puts in, drawn under its row.
    rig.app.click("role:button \"Try it\"");
    rig.frames(10);
    let tried = rig.app.text("snippets-tried");
    assert!(tried.starts_with("Standup for ") && !tried.contains("{date}"), "{tried}");
    // A field that isn't one is said under the text, nothing kept while it is.
    rig.app.fill("role:textbox \"Text\"", "Standup for {weekday}");
    rig.frames(10);
    assert!(rig.tree().contains("{weekday} isn't a field. Use {date} or {time}."), "{}", rig.tree());
    assert!(rig.app.find("#snippets-tried").is_none(), "what was tried goes with the change");
    rig.app.keys("tab");
    rig.frames(50);
    assert!(kept(&rig).is_null(), "nothing kept while it's wrong (the starters, as they came): {}", kept(&rig));
    // A new one: its row, open, named so no other is.
    rig.app.click("role:button \"New snippet\"");
    rig.frames(20);
    assert_eq!(kept(&rig).as_array().unwrap().len(), 3);
    assert!(rig.line("textbox \"Name\"").contains("New snippet"), "{}", rig.tree());
    // A name another has is said, and not kept; one of its own is.
    rig.app.fill("role:textbox \"Name\"", "standup");
    rig.app.keys("enter");
    rig.frames(10);
    assert!(rig.tree().contains("Another snippet has this name."));
    assert_eq!(kept(&rig)[2]["name"], json!("New snippet"));
    rig.app.fill("role:textbox \"Name\"", "Thanks");
    rig.app.keys("enter");
    rig.frames(10);
    assert_eq!(kept(&rig)[2]["name"], json!("Thanks"));
    assert!(rig.app.find("#snippets-list >> \"Thanks\"").is_some(), "its row follows");
    // Deleted, after asking: the one beside it opens.
    rig.app.click("role:button \"Delete\"");
    rig.frames(10);
    assert!(rig.app.find("role:button \"Delete snippet\"").is_some());
    rig.app.click("role:button \"Delete snippet\"");
    rig.frames(20);
    assert_eq!(kept(&rig).as_array().unwrap().len(), 2);
    assert!(rig.app.find("#snippets-list >> \"Thanks\"").is_none());
    assert!(rig.heard.borrow().iter().any(|(k, v)| k == "plugin.settings-page.toast" && v.as_str().is_some_and(|t| t.contains("Deleted “Thanks”"))));
    assert_eq!(rig.app.error(), None);
}

#[test]
fn settings_search_finds_a_page_by_its_words_and_lands_on_it() {
    let mut rig = Rig::new(&[example("settings-page")]);
    rig.frames(10);
    assert!(rig.app.host.run_command("settings_stub.sign_in"));
    assert!(rig.app.host.run_command("shell.settings"));
    rig.frames(20);
    rig.app.type_text(&format!("{NAV} >> role:textbox \"Search settings\""), "templates");
    rig.frames(10);
    assert!(rig.app.find("#settings-search-results >> role:option \"Snippets\"").is_some(), "{}", rig.tree());
    rig.app.click("#settings-search-results >> role:option \"Snippets\"");
    rig.frames(30);
    assert!(rig.app.find("#snippets-list").is_some(), "landed on its page: {}", rig.tree());
}

/// A page with nothing declared, binding a setting it declares no row of
/// isn't possible; one declaring a switch and binding it is: its own control
/// and Zeron's row move together, and its listener hears each change once.
fn binder() -> PathBuf {
    written(
        "binder",
        json!({ "id": "binder", "name": "Binder", "version": "1.0.0", "entry": "init.luau", "permissions": [],
                "settings": [
                    { "id": "loud", "kind": "switch", "label": "Loud", "default": false },
                    { "id": "level", "kind": "number", "label": "Level", "min": 1, "max": 5, "default": 2 },
                    { "id": "dir", "kind": "folder", "label": "Postcards" },
                ] }),
        r#"
local zeron = require("./zeron")("binder")
local Switch = require("@ds/switch")
local Button = require("@ds/button")
zeron.on_setting(function(id, value)
	zeron.toast(`{id}={tostring(value)}`)
end)
zeron.settings_page {
	build = function(page)
		return ui.column {
			id = "binder-page",
			Switch { value = page.bind("loud"), label = "Loud too" },
			Button { "Too loud", on_click = function() zeron.setting("level"):set(99) end },
			Button { "Bad level", on_click = function() zeron.setting("level"):set("high") end },
			Button { "Sneak a folder", on_click = function() zeron.setting("dir"):set("/") end },
			Button { "Choose a folder", on_click = function() page.choose_folder("dir") end },
			Button { "Another's", on_click = function() ui.call("plugin.other.setting_set", { id = "tone", value = "dry" }) end },
		}
	end,
}
"#,
    )
}

fn other() -> PathBuf {
    written(
        "other",
        json!({ "id": "other", "name": "Other", "version": "1.0.0", "entry": "init.luau", "permissions": [],
                "settings": [{ "id": "tone", "kind": "choice", "label": "Tone", "options": [{ "value": "warm" }, { "value": "dry" }], "default": "warm" }] }),
        "local zeron = require(\"./zeron\")(\"other\")\n",
    )
}

#[test]
fn a_page_sets_its_own_values_alone_each_checked_and_heard_once() {
    let mut rig = Rig::new(&[binder(), other()]);
    rig.open("Binder");
    assert!(rig.app.find("#binder-page").is_some(), "{}", rig.tree());
    let toasts = |rig: &Rig| -> Vec<String> { rig.heard.borrow().iter().filter(|(k, _)| k == "plugin.binder.toast").filter_map(|(_, v)| v.as_str().map(str::to_string)).collect() };
    // Its own switch: one value with Zeron's row, heard once.
    rig.app.click("#binder-page >> role:switch \"Loud too\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "loud"), json!(true));
    assert!(rig.line("switch \"Loud\"").contains("checked"), "Zeron's row follows: {}", rig.tree());
    assert_eq!(toasts(&rig), ["loud=true"]);
    // Zeron's row: its control follows.
    rig.app.click(&format!("{PAGE} >> role:switch \"Loud\""));
    rig.frames(10);
    assert_eq!(rig.value("binder", "loud"), json!(false));
    assert!(rig.line("switch \"Loud too\"").contains("unchecked"), "{}", rig.tree());
    // Checked as the row checks: within its bounds; what doesn't fit refused.
    rig.app.click("role:button \"Too loud\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "level"), json!(5.0));
    rig.app.click("role:button \"Bad level\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "level"), json!(5.0));
    // A folder is the person's: refused set, chosen in the dialog.
    rig.app.click("role:button \"Sneak a folder\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "dir"), json!(""));
    rig.app.click("role:button \"Choose a folder\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "dir"), json!("/Users/me/Picked"));
    // Another plugin's values are out of its reach.
    rig.app.click("role:button \"Another's\"");
    rig.frames(10);
    assert_eq!(rig.value("other", "tone"), json!("warm"));
    assert!(!rig.heard.borrow().iter().any(|(k, _)| k == "plugin.other.setting_set"), "never reached");
}

#[test]
fn a_build_that_fails_is_said_in_its_place_and_the_declared_rows_stay() {
    let dir = written(
        "broken",
        json!({ "id": "broken", "name": "Broken", "version": "1.0.0", "entry": "init.luau", "permissions": [],
                "settings": [{ "id": "on", "kind": "switch", "label": "Keep going", "default": true }] }),
        r#"
local zeron = require("./zeron")("broken")
zeron.settings_page {
	build = function(page)
		local counts = nil
		return ui.text { tostring(counts.total) }
	end,
}
"#,
    );
    let mut rig = Rig::new(&[dir]);
    rig.open("Broken");
    assert_eq!(rig.app.error(), None, "contained: Settings goes on");
    let problem = rig.book.borrow().page_problem("broken").flatten().unwrap_or_default();
    assert!(problem.contains("attempt to index nil"), "{problem}");
    let tree = rig.tree();
    assert!(tree.contains("Broken's settings couldn't be shown"), "{tree}");
    assert!(tree.contains("attempt to index nil"), "{tree}");
    assert!(rig.app.find(&format!("{PAGE} >> role:switch \"Keep going\"")).is_some(), "its declared rows still there");
    // Settings works on: another page, and back.
    rig.app.click(&format!("{NAV} >> role:tab \"General\""));
    rig.frames(20);
    assert_eq!(rig.app.error(), None);
}

#[test]
fn a_build_that_runs_away_is_stopped_and_its_page_says_so_its_rows_still_there() {
    let dir = written(
        "spin",
        json!({ "id": "spin", "name": "Spin", "version": "1.0.0", "entry": "init.luau", "permissions": [],
                "settings": [{ "id": "fast", "kind": "switch", "label": "Go faster", "default": false }] }),
        r#"
local zeron = require("./zeron")("spin")
zeron.settings_page {
	build = function(page)
		while true do end
	end,
}
"#,
    );
    let mut rig = Rig::new(&[dir]);
    rig.open("Spin");
    // zgx stops it within its budget: the window goes on, the error the
    // plugin's, which the bridge blames on it and withdraws it for.
    let error = rig.app.error().unwrap_or_default();
    assert!(error.starts_with("spin: ") && error.contains("time budget"), "{error}");
    let loaded: Vec<zgx_script::Plugin> = rig.items.iter().filter_map(Item::plugin).collect();
    assert_eq!(super::recovery::blame(&error, &loaded).as_deref(), Some("spin"));
    rig.app.host.set_plugins(Vec::new());
    rig.app.host.reload().unwrap();
    // As the bridge follows it then: withdrawn, but it drew a page of its
    // own, so its page stays, saying why.
    let why = super::recovery::reason(&error);
    let (on, stopped) = {
        let book = rig.book.borrow();
        super::settings_on(&rig.items, &Default::default(), |_| Some(why.clone()), |id| book.page_problem(id).is_some())
    };
    assert_eq!(stopped, [("spin".to_string(), why.clone())]);
    rig.book.borrow_mut().sync(&on, |_| manifest_in(Path::new(rig.items[0].json["snapshot"].as_str().unwrap())));
    rig.book.borrow_mut().page_built("spin", Some(&why));
    rig.publish();
    rig.frames(30);
    let tree = rig.tree();
    assert!(tree.contains("Spin's settings couldn't be shown"), "{tree}");
    assert!(tree.contains("time budget"), "{tree}");
    assert!(rig.app.find(&format!("{PAGE} >> role:switch \"Go faster\"")).is_some(), "its declared rows still there: {tree}");
}

#[test]
fn a_withdrawn_plugin_keeps_its_settings_only_if_it_drew_its_own_page() {
    let items = vec![item(&example("settings-page")), item(&example("settings-demo"))];
    let failed = |i: &Item| Some(format!("{} broke", i.id()));
    // Loading: both followed, neither stopped.
    let loading: std::collections::HashSet<String> = items.iter().map(|i| i.id().to_string()).collect();
    let (on, stopped) = super::settings_on(&items, &loading, failed, |_| true);
    assert_eq!((on.len(), stopped.len()), (2, 0));
    // Withdrawn: the one with its own page stays, saying why; the one with
    // declared rows alone goes, as every withdrawn plugin's page always has.
    let (on, stopped) = super::settings_on(&items, &Default::default(), failed, |id| id == "settings-page");
    assert_eq!(on.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["settings-page"]);
    assert_eq!(stopped, [("settings-page".to_string(), "settings-page broke".to_string())]);
    // Not failed, only off: nothing.
    let (on, _) = super::settings_on(&items, &Default::default(), |_| None, |_| true);
    assert!(on.is_empty());
}

#[test]
fn turning_the_plugin_off_takes_its_page_and_its_place_in_settings() {
    let mut rig = Rig::new(&[example("settings-page")]);
    rig.open("Snippets");
    assert!(rig.app.find("#snippets-list").is_some());
    // Off: the book lets go of it, as the bridge's sync does.
    rig.book.borrow_mut().sync(&[], |_| None);
    rig.publish();
    rig.frames(20);
    assert!(rig.app.find(&format!("{NAV} >> role:tab \"Snippets\"")).is_none());
    assert!(rig.app.find("#snippets-list").is_none());
    assert!(rig.app.find(OWN).is_none());
    assert_eq!(rig.app.error(), None);
}

#[test]
fn another_plugin_reaches_nothing_of_a_plugin_s_own_page() {
    // A plugin allowed to add and change anywhere it names, aiming at what
    // Snippets draws on its page, and at its box.
    let hostile = written(
        "hostile",
        json!({ "id": "hostile", "name": "Hostile", "version": "1.0.0", "entry": "init.luau",
                "permissions": ["ui.add", "ui.change"],
                "ui": { "change": ["#snippets-list", "role:button \"New snippet\"", "#plugin-settings-page"] } }),
        r##"
local zeron = require("./zeron")("hostile")
for _, at in { "#snippets-list", 'role:button "New snippet"' } do
	zeron.place { at = at, replace = function() return ui.text { id = "hostile-replaced", "Gotcha" } end }
	zeron.place { at = at, after = function() return ui.text { id = "hostile-added", "Gotcha" } end }
end
-- Its own page given to the slot: shown in its own box alone, never another's.
ui.contribute("plugin_page", ui.text { id = "hostile-page", "Not yours" })
-- Last (zgx refuses it, failing what's after it): the box, replaced whole.
ui.contribute { target = "#plugin-settings-page", replace = ui.text { id = "hostile-box", "Gotcha" } }
"##,
    );
    let mut rig = Rig::new(&[example("settings-page"), hostile]);
    rig.open("Snippets");
    for id in ["hostile-replaced", "hostile-added", "hostile-page", "hostile-box"] {
        assert!(rig.app.find(&format!("#{id}")).is_none(), "{id} reached Snippets' page: {}", rig.tree());
    }
    // zgx refuses to take the box away; what Snippets drew is all there.
    assert!(rig.app.error().is_some_and(|e| e.starts_with("hostile:") && e.contains("decision")), "{:?}", rig.app.error());
    assert!(rig.app.find("#snippets-list >> role:button \"Standup\"").is_some(), "{}", rig.tree());
    assert!(rig.app.find("role:button \"New snippet\"").is_some());
}

/// A plugin started with a settings page of its own (`create_plugin {
/// settings_page = true }`: the engine's scaffold writes plugins-sdk's
/// settings_page.luau, named for it, and its words).
fn scaffolded() -> PathBuf {
    let page = include_str!("../../plugins-sdk/settings_page.luau").replace("PLUGIN_ID", "ledger").replace("PLUGIN_NAME", "Ledger");
    let dir = written(
        "ledger",
        json!({ "id": "ledger", "name": "Ledger", "version": "0.1.0", "entry": "init.luau", "permissions": [] }),
        "local zeron = require(\"./zeron\")(\"ledger\")\nrequire(\"./settings_page\")\n",
    );
    std::fs::write(dir.join("settings_page.luau"), page).unwrap();
    std::fs::create_dir_all(dir.join("locales")).unwrap();
    std::fs::write(dir.join("locales/en.json"), include_str!("../../plugins-sdk/settings_page.en.json")).unwrap();
    dir
}

#[test]
fn a_scaffolded_settings_page_is_right_from_the_start() {
    let dir = scaffolded();
    // Nothing in it off the design system, as in the example to copy.
    assert_eq!(super::surfaces::style_hints(&dir), (vec![], 0));
    assert_eq!(super::surfaces::style_hints(&example("settings-page")), (vec![], 0), "the example is on the system throughout");
    let mut rig = Rig::new(&[dir]);
    rig.open("Ledger");
    assert_eq!(rig.app.error(), None);
    assert_eq!(rig.book.borrow().page_problem("ledger"), Some(None), "it built");
    // Its words, its empty state in the card's place, its action at the
    // section title's end; an item added is its row.
    let tree = rig.tree();
    assert!(tree.contains("heading \"Items\"") || tree.contains("\"Items\""), "{tree}");
    assert!(tree.contains("No items yet"), "{tree}");
    rig.app.click("role:button \"Add item\"");
    rig.frames(20);
    assert!(rig.tree().contains("New item 1"), "{}", rig.tree());
    assert!(rig.app.find("role:button \"Remove New item 1\"").is_some());
    rig.app.click("role:button \"Remove New item 1\"");
    rig.frames(20);
    assert!(rig.tree().contains("No items yet"));
}

/// The guide's model page (docs/plugins.md, "A model page"), as it's
/// written there.
fn guides_model_page() -> PathBuf {
    let guide = include_str!("../../docs/plugins.md");
    let from = guide.find("### A model page").expect("the guide has a model page");
    let code = &guide[from..];
    let start = code.find("```lua\n").unwrap() + "```lua\n".len();
    let code = &code[start..start + code[start..].find("```").unwrap()];
    written(
        "feeds",
        json!({ "id": "feeds", "name": "Feeds", "version": "1.0.0", "entry": "init.luau", "permissions": [],
                "settings": [{ "id": "every", "kind": "number", "label": "Check every", "min": 5, "max": 120, "unit": "minutes", "default": 15 }] }),
        code,
    )
}

#[test]
fn the_guide_s_model_page_works_as_written() {
    let dir = guides_model_page();
    assert_eq!(super::surfaces::style_hints(&dir), (vec![], 0), "on the system throughout");
    let mut rig = Rig::new(&[dir]);
    rig.open("Feeds");
    assert_eq!(rig.app.error(), None);
    assert_eq!(rig.book.borrow().page_problem("feeds"), Some(None), "it built");
    assert!(rig.app.find(&format!("{PAGE} >> role:textbox \"Check every\"")).is_some(), "its declared row first");
    assert!(rig.tree().contains("No feeds yet"));
    // Added: its row, and it open under the list.
    rig.app.click("role:button \"Add feed\"");
    rig.frames(20);
    assert!(rig.app.find("#feed-editor").is_some(), "{}", rig.tree());
    assert!(rig.tree().contains("No address yet."));
    // A wrong address is said, and not kept; a right one is.
    rig.app.fill("#feed-editor >> role:textbox \"Address\"", "http://example.com/feed");
    rig.app.keys("enter");
    rig.frames(10);
    assert!(rig.tree().contains("It starts with https://."));
    rig.app.fill("#feed-editor >> role:textbox \"Address\"", "https://example.com/feed");
    rig.app.keys("enter");
    rig.frames(10);
    assert!(rig.tree().contains("https://example.com/feed"), "its row says it");
    // Quiet hours: a row comes with the switch, and goes with it.
    assert!(!rig.tree().contains("Overnight"));
    rig.app.click("#feed-editor >> role:switch \"Quiet hours\"");
    rig.frames(10);
    assert!(rig.tree().contains("Overnight"));
    // Removed: the list empty again.
    rig.app.click("#feed-editor >> role:button \"Remove\"");
    rig.frames(20);
    assert!(rig.app.find("#feed-editor").is_none());
    assert!(rig.tree().contains("No feeds yet"));
    assert_eq!(rig.app.error(), None);
}
