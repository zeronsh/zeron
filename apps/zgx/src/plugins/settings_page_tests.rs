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
const OWN: &str = ".plugin-settings-page";

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
                    let pace = Rc::new(RefCell::new(super::settings::Pace::default()));
                    host.async_action(format!("plugin.{id}.setting_set"), move |v: Value| {
                        let asked = value_json(&v);
                        heard.borrow_mut().push((format!("plugin.{id}.setting_set"), asked.clone()));
                        // (Paced as the bridge paces it.)
                        let paced = pace.borrow_mut().allow(&id, std::time::Instant::now());
                        let done = if paced { book.borrow_mut().set_own(&id, asked["id"].as_str().unwrap_or_default(), &asked["value"]) } else { Err("too many".to_string()) };
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
                    let route = host.shared("zeron.route", json_value(&json!({ "name": "home" })));
                    host.async_action(format!("plugin.{id}.setting_folder"), move |v: Value| {
                        let asked = value_json(&v);
                        let setting = asked["id"].as_str().unwrap_or_default().to_string();
                        // (As the bridge: from its settings page, while it shows.)
                        let shown = super::settings::page_shown(&value_json(&route.signal().with_untracked(Value::clone)), &id);
                        let label = if shown { book.borrow().folder_label(&id, &setting) } else { Err("not from its page".to_string()) };
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

    /// How many nodes `target` names.
    fn count(&mut self, target: &str) -> usize {
        self.app.frames(1);
        let target = zgx_script::driver::parse(target).unwrap();
        zgx_script::driver::find_all(&self.app.ui, &self.app.handle, &target).map(|n| n.len()).unwrap_or(0)
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
    // As wide as Zeron's cards, and its place as tall as what it drew.
    let card = rig.app.bounds(&format!("{PAGE} >> role:group \"Message box\""));
    assert!((drawn.width - card.width).abs() < 1. && (drawn.x - card.x).abs() < 1., "{card:?} {drawn:?}");
    assert!(own.height >= drawn.height, "its place holds it: {own:?} {drawn:?}");
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
    // A text snippet has no Try it; its card a hairline between each of its
    // three rows, once.
    assert!(rig.app.find("role:button \"Try it\"").is_none());
    assert_eq!(rig.count("#snippets-editor >> .settings-rule"), 2);
    // Its count reads out of the real limit.
    assert!(rig.tree().contains("/2000") && !rig.tree().contains("/1800"), "{}", rig.tree());
    // Another chosen: open under the list; the list's rows stay as they were.
    rig.app.click("#snippets-list >> role:button \"Standup\"");
    rig.frames(20);
    assert!(rig.line("textbox \"Name\"").contains("Standup"));
    assert_eq!(rig.app.find("#snippets-list >> role:button \"Ask for a review\""), review_row, "the list's rows, in place");
    // A template's Try it comes with its own hairline, and only one.
    assert_eq!(rig.count("#snippets-editor >> .settings-rule"), 3);
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
    // Nobody has typed in its empty text yet: nothing said against it.
    assert!(!rig.tree().contains("Write what it puts in the message box."), "{}", rig.tree());
    rig.app.fill("role:textbox \"Text\"", "x");
    rig.app.fill("role:textbox \"Text\"", "");
    rig.frames(10);
    assert!(rig.tree().contains("Write what it puts in the message box."), "once typed in, it says so: {}", rig.tree());
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
			Button { "Flood", on_click = function()
				for i = 1, 60 do zeron.setting("loud"):set(i % 2 == 0) end
			end },
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
    assert_eq!(rig.value("binder", "level"), json!(2.0), "past its bounds: refused, as its row refuses it");
    rig.app.click("role:button \"Bad level\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "level"), json!(2.0));
    assert_eq!(rig.app.error(), None, "a refused set is said as the plugin's, never left to stop it");
    // A folder is the person's: refused set, chosen in the dialog.
    rig.app.click("role:button \"Sneak a folder\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "dir"), json!(""));
    rig.app.click("role:button \"Choose a folder\"");
    rig.frames(10);
    assert_eq!(rig.value("binder", "dir"), json!("/Users/me/Picked"));
    // A flood of changes is paced: so many a second, the rest refused.
    let asked_before = rig.heard.borrow().iter().filter(|(k, _)| k == "plugin.binder.setting_set").count();
    rig.app.click("role:button \"Flood\"");
    rig.frames(10);
    let asked = rig.heard.borrow().iter().filter(|(k, _)| k == "plugin.binder.setting_set").count() - asked_before;
    assert_eq!(asked, 60, "every one asked");
    assert_eq!(rig.app.error(), None, "the refused ones are its page's to hear, not Settings'");
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
    // Withdrawn, it draws nothing: its place is Zeron's own again, empty.
    assert!(rig.app.find(".plugin-settings.plugin-spin").is_some(), "its place, not filled: {tree}");
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
    // Snippets draws on its page, and at its placeholder.
    let hostile = written(
        "hostile",
        json!({ "id": "hostile", "name": "Hostile", "version": "1.0.0", "entry": "init.luau",
                "permissions": ["ui.add", "ui.change"],
                "ui": { "change": ["#snippets-list", "role:button \"New snippet\""] } }),
        r##"
local zeron = require("./zeron")("hostile")
for _, at in { "#snippets-list", 'role:button "New snippet"' } do
	zeron.place { at = at, replace = function() return ui.text { id = "hostile-replaced", "Gotcha" } end }
	zeron.place { at = at, after = function() return ui.text { id = "hostile-added", "Gotcha" } end }
end
-- Another's placeholder, filled: zgx refuses it (only its plugin may).
local ok, why = pcall(ui.contribute, { target = ".plugin-settings.plugin-settings-page", replace = ui.text { id = "hostile-page", "Not yours" } })
zeron.toast(if ok then "filled" else `refused: {why}`)
"##,
    );
    let mut rig = Rig::new(&[example("settings-page"), hostile]);
    rig.open("Snippets");
    for id in ["hostile-replaced", "hostile-added", "hostile-page"] {
        assert!(rig.app.find(&format!("#{id}")).is_none(), "{id} reached Snippets' page: {}", rig.tree());
    }
    let toast = rig.heard.borrow().iter().find(|(k, _)| k == "plugin.hostile.toast").map(|(_, v)| v.as_str().unwrap_or_default().to_string()).unwrap_or_default();
    assert!(toast.starts_with("refused"), "{toast}");
    // What Snippets drew is all there.
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

/// A settings page can do what a page of its own (`zeron.page`) can: the same
/// code on both, each overlay opened in turn (a dialog over the window, the
/// lightbox, a popover wider than the page's column, a select's menu, a
/// tooltip, a hover card), and as much of each shows on one as on the other.
/// `--nocapture` prints the table.
#[test]
fn a_settings_page_opens_whatever_a_page_of_its_own_does() {
    let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/assets/favicons/flint.png");
    let widgets = format!(
        r##"
local Dialog = require("@ds/dialog").Dialog
local Lightbox = require("@ds/lightbox")
local Select = require("@ds/select")
local Button = require("@ds/button")
local HoverCard = require("@ds/hover_card")
local ContextMenu = require("@ds/context_menu")
local PNG = "{png}"
return function(p: string): Node
	local dialog = ui.use_state(false)
	local pop = ui.use_state(false)
	local choice = ui.use_state("o1")
	local opts = {{}}
	for i = 1, 14 do table.insert(opts, {{ value = `o{{i}}`, label = `Option {{i}}` }}) end
	return ui.column {{ id = `{{p}}-probe`, w_full = true, gap = "$space.2",
		Lightbox.Host(),
		Button {{ `Dialog {{p}}`, on_click = function() dialog:set(true) end }},
		Dialog {{ open = dialog, title = `Delete {{p}}?`, body = "It can't be undone.", tone = "danger", confirm = {{ label = "Delete", on_click = function() end }} }},
		Button {{ `Picture {{p}}`, on_click = function() Lightbox.open({{ {{ src = PNG, name = "flint", w = 512, h = 512 }} }}, 1) end }},
		ui.popover {{ open = pop, label = `Wide {{p}}`, anchor = Button {{ `Popover {{p}}`, on_click = function() pop:set(true) end }},
			ui.div {{ id = `{{p}}-wide`, w = 1000, h = 120 }} }},
		ui.row {{ w_full = true, justify = "end", Select {{ value = choice, options = opts, name = `Select {{p}}`, w = 168 }} }},
		ui.div {{ id = `{{p}}-tip`, w = 60, h = 20, tooltip = "A tip, the same on both" }},
		ContextMenu {{ items = function() return {{ {{ label = "Copy the same on both", on_select = function() end }} }} end,
			Button {{ `Context {{p}}` }} }},
		ui.div {{ id = `{{p}}-card`, w = 60, h = 20, tooltip = HoverCard.Card {{ id = `{{p}}-card-body`, w = 520, h = 160 }} }},
	}}
end
"##,
        png = "flint.png"
    );
    let dir = written(
        "probe",
        json!({ "id": "probe", "name": "Probe", "version": "1.0.0", "entry": "init.luau", "permissions": ["pages.add"],
                "settings": [{ "id": "on", "kind": "switch", "label": "On", "default": true }] }),
        r#"
local zeron = require("./zeron")("probe")
local widgets = require("./widgets")
zeron.page { id = "probe", title = "Probe", build = function() return ui.column { w_full = true, h_full = true, widgets("page") } end }
zeron.settings_page { build = function() return widgets("settings") end }
"#,
    );
    std::fs::write(dir.join("widgets.luau"), widgets).unwrap();
    std::fs::copy(&png, dir.join("flint.png")).unwrap();
    let mut rig = Rig::new(&[dir]);
    rig.frames(10);
    // How much of what `shows` names shows, once `open` is clicked (or, with
    // `hover`, rested on twice: a window's first tooltip waits a frame more).
    // (`keys`: focused, then the Menu key's Shift+F10, as a context menu
    // opens from the keys.)
    fn shown(rig: &mut Rig, open: &str, shows: &str, hover: bool) -> Option<(f32, f32)> {
        let rounds = if hover { 2 } else { 1 };
        let mut out = None;
        let (open, keys) = match open.strip_prefix("keys:") {
            Some(at) => (at, true),
            None => (open, false),
        };
        for _ in 0..rounds {
            if keys {
                rig.app.click(open);
                rig.frames(10);
                rig.app.keys("shift+f10");
                rig.frames(40);
            } else if hover {
                rig.app.hover(open);
                rig.frames(120);
                std::thread::sleep(std::time::Duration::from_millis(900));
                rig.frames(60);
            } else {
                rig.app.click(open);
                rig.frames(40);
            }
            out = rig.app.find(shows).and_then(|n| rig.app.ui.scene.borrow().visible_bounds(n)).map(|v| (v.width.round(), v.height.round()));
            if hover {
                rig.app.hover("#shell");
                rig.frames(30);
            } else {
                rig.app.keys("escape");
                rig.frames(40);
            }
        }
        out
    }
    let cases = |side: &str| -> Vec<(String, String, String, bool)> {
        vec![
            ("a dialog".into(), format!("role:button \"Dialog {side}\""), format!("role:dialog \"Delete {side}?\""), false),
            ("the lightbox".into(), format!("role:button \"Picture {side}\""), "role:dialog".into(), false),
            ("a popover wider than the column".into(), format!("role:button \"Popover {side}\""), format!("#{side}-wide"), false),
            ("a select's menu".into(), format!("role:button \"Select {side}\""), "role:menuitem \"Option 1\"".into(), false),
            ("a tooltip".into(), format!("#{side}-tip"), "\"A tip, the same on both\"".into(), true),
            ("a hover card".into(), format!("#{side}-card"), format!("#{side}-card-body"), true),
            ("a context menu".into(), format!("keys:role:button \"Context {side}\""), "role:menuitem \"Copy the same on both\"".into(), false),
        ]
    };
    let mut got: Vec<Vec<Option<(f32, f32)>>> = Vec::new();
    for side in ["settings", "page"] {
        if side == "page" {
            rig.app.host.shared("plugins.pages", Value::Nil).set(json_value(&json!([{ "plugin": "probe", "plugin_name": "Probe", "id": "probe", "title": "Probe" }])));
            rig.frames(10);
            rig.app.host.run_command("router.back");
            rig.frames(30);
            rig.app.click(".plugin-page-row");
            rig.frames(40);
        } else {
            rig.open("Probe");
        }
        got.push(cases(side).iter().map(|(_, open, shows, hover)| shown(&mut rig, open, shows, *hover)).collect());
        assert_eq!(rig.app.error(), None, "{side}");
    }
    let names: Vec<String> = cases("settings").into_iter().map(|c| c.0).collect();
    for (i, name) in names.iter().enumerate() {
        eprintln!("{name:36} settings page {:?}   its own page {:?}", got[0][i], got[1][i]);
    }
    for (i, name) in names.iter().enumerate() {
        assert!(got[1][i].is_some(), "{name} shows on a page of its own");
        assert_eq!(got[0][i], got[1][i], "{name}: as much shows on its settings page as on a page of its own");
    }
    // Over the window, not cut to the page: the dialog whole, the lightbox
    // the window's size.
    assert_eq!(got[0][0], Some((380., 152.)));
    assert_eq!(got[0][1], Some((1200., 1400.)));
}

/// What the person sets that the design system follows reaches the copy lent
/// to a plugin, at once, set through Zeron's own controls with the plugin's
/// page open in the window: frosted glass, the accent, the accent throughout,
/// reduce motion, the text size, hiding email addresses (ds/preference). And
/// a plugin can't change one.
#[test]
fn what_the_person_sets_in_appearance_reaches_a_plugin_s_design_system() {
    let dir = written(
        "looks",
        json!({ "id": "looks", "name": "Looks", "version": "1.0.0", "entry": "init.luau", "permissions": [] }),
        r#"
local zeron = require("./zeron")("looks")
local material = require("@ds/material")
local motion = require("@ds/motion")
local look = require("@ds/look")
local PrivateText = require("@ds/private_text")
local Button = require("@ds/button")
local function said(id, read)
	return ui.text { id = id, function() return tostring(read()) end }
end
zeron.settings_page {
	build = function()
		return ui.column {
			id = "looks-page",
			said("looks-frosted", function() return material.frosted:get() end),
			said("looks-accent", function() return look.accent:get() end),
			said("looks-accented", function() return look.accented:get() end),
			said("looks-reduce", function() return motion.reduce:get() end),
			said("looks-text-size", function() return look.text_size:get() end),
			said("looks-hidden", function() return PrivateText.hidden:get() end),
			Button { "Change it", on_click = function()
				local ok = pcall(function() material.frosted:set(not material.frosted:peek()) end)
				zeron.toast(if ok then "changed" else "refused")
			end },
		}
	end,
}
"#,
    );
    let mut rig = Rig::new(&[dir]);
    let shown = |rig: &mut Rig| -> Vec<String> {
        ["looks-frosted", "looks-accent", "looks-accented", "looks-reduce", "looks-text-size", "looks-hidden"].iter().map(|id| rig.app.text(id)).collect()
    };
    let go = |rig: &mut Rig, page: &str| {
        rig.app.scroll(NAV, -2000.);
        rig.frames(5);
        rig.app.scroll(NAV, if page == "Looks" { 2000. } else { 0. });
        rig.frames(5);
        rig.app.click(&format!("{NAV} >> role:tab \"{page}\""));
        rig.frames(30);
    };
    rig.open("Looks");
    let before = shown(&mut rig);
    // (The fixture's own: hide emails off, the rest their defaults.)
    assert_eq!(before, ["true", "default", "true", "false", "13", "false"], "the app's values, not the plugin's own defaults");
    // Each through Zeron's own control, then back to the plugin's page.
    go(&mut rig, "Appearance");
    rig.app.click(&format!("{PAGE} >> role:switch \"Frosted glass\""));
    rig.app.click(&format!("{PAGE} >> role:switch \"Accent color throughout\""));
    rig.app.click(&format!("{PAGE} >> role:switch \"Reduce motion\""));
    rig.app.click(&format!("{PAGE} >> role:radio \"Green\""));
    rig.frames(10);
    rig.app.click(&format!("{PAGE} >> role:button \"Interface text size\""));
    rig.frames(20);
    rig.app.click("role:menuitem \"15 pt\"");
    rig.frames(10);
    go(&mut rig, "General");
    rig.app.click(&format!("{PAGE} >> role:switch \"Hide email addresses\""));
    rig.frames(10);
    go(&mut rig, "Looks");
    assert_eq!(shown(&mut rig), ["false", "green", "false", "true", "15", "true"], "{}", rig.tree());
    // A plugin can't change one: refused, the app's as it was.
    rig.app.click("role:button \"Change it\"");
    rig.frames(10);
    let toast = rig.heard.borrow().iter().rev().find(|(k, _)| k == "plugin.looks.toast").map(|(_, v)| v.clone());
    assert_eq!(toast, Some(json!("refused")));
    assert_eq!(rig.app.text("looks-frosted"), "false");
    assert_eq!(rig.app.error(), None);
}
