//! Plugins run for real, headless: an app with Zeron's surface (adding
//! anywhere behind "ui.add", what plugins may read, each plugin's own calls
//! and records) and plugins granted what `Item::plugin` maps their
//! permissions to. What a plugin wasn't allowed it can't do, `hook = false`
//! is out of reach whatever it was allowed, a placed node reads only the
//! fields its grants allow, one that needs what its place doesn't give
//! draws nothing and says so, and the SDK's tools and messages work over
//! the keys the bridge keeps.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use serde_json::json;
use zgx_script::{ScriptHost, Value, testing::TestApp};

use super::{Item, expose_own, expose_surface, json_value, live, problem_words, records, recovery, vlist};

const APP: &str = r#"
local who = ui.shared("zeron.route", { name = "home" })
local rows = ui.state("rows", { { id = "s1" }, { id = "s2" } })
return ui.column {
    id = "root",
    ui.row { id = "bar", ui.button { id = "add", "Add" } },
    ui.row { id = "account", ui.button { id = "signout", hook = false, "Sign out" } },
    ui.text { id = "title", "Hello" },
    ui.text { id = "route", function() return who:get().name end },
    -- A list's rows, each giving its session, as the sidebar's do.
    ui.column {
        id = "list",
        ui.each(rows, function(r)
            return ui.row {
                id = "row-" .. r.id,
                class = "session-row",
                provide = { session = ui.computed(function() return r.id end) },
                ui.row { class = "session-title", ui.text { "Row " .. r.id } },
            }
        end, { key = function(r) return r.id end }),
    },
    -- A plugin's tab, as the panel draws one: what's around it gives the tab.
    ui.column {
        provide = { tab = { kind = "pr", title = "PR 12", data = { n = 12 } } },
        ui.column { id = "tabslot", class = "plugin-tab plugin-prs", ui.text { "placeholder" } },
    },
}
"#;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-plugin-loading-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A plugin `id` on, from a snapshot holding `init` (and the SDK), asking
/// `permissions`.
fn plugin(root: &Path, id: &str, init: &str, permissions: &[&str], change: &[&str], replace: &[&str]) -> Item {
    let snapshot = root.join("plugins").join(id);
    write(&snapshot, "init.luau", init);
    write(&snapshot, "zeron.luau", include_str!("../../plugins-sdk/zeron.luau"));
    Item {
        json: json!({
            "id": id, "name": id, "status": "on", "entry": "init.luau",
            "snapshot": snapshot, "permissions": permissions,
            "ui": { "change": change, "replace": replace },
        }),
    }
}

/// The app with `items` loaded, and what their own actions heard.
fn app(root: &Path, items: &[Item], extra: Vec<zgx_script::Plugin>) -> (TestApp, Rc<RefCell<Vec<(String, String)>>>) {
    app_of(root, APP, items, extra)
}

/// `app`, with the app's script `source`.
fn app_of(root: &Path, source: &str, items: &[Item], extra: Vec<zgx_script::Plugin>) -> (TestApp, Rc<RefCell<Vec<(String, String)>>>) {
    write(root, "app/app.luau", source);
    let heard: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    let entry = root.join("app/app.luau");
    let items = items.to_vec();
    let log = heard.clone();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, &entry);
        expose_surface(&host);
        let mut plugins = Vec::new();
        for item in &items {
            let id = item.id().to_string();
            expose_own(&host, &id);
            host.shared(&format!("plugin.{id}.calls"), vlist(Vec::new()));
            host.shared(&format!("plugin.{id}.inbox"), vlist(Vec::new()));
            host.shared(&format!("plugin.{id}.records"), Value::Nil).set(json_value(&records::records_with(&item.permissions(), &item.change(), &sources())));
            for name in ["result", "ack", "menus", "toast", "problem", "open_tab", "add_page", "cost"] {
                let (log, key) = (log.clone(), format!("plugin.{id}.{name}"));
                host.action(key.clone(), move |v: Value| log.borrow_mut().push((key.clone(), v.to_json())));
            }
            plugins.extend(item.plugin());
        }
        plugins.extend(extra);
        host.set_plugins(plugins);
        host
    });
    (app, heard)
}

/// What records are made from, in these tests.
fn sources() -> records::Sources {
    records::Sources {
        sessions: json!({
            "chats": [{ "id": "s1", "title": "Fix the build", "status": "working", "cwd": "/secret" }],
            "projects": [
                { "id": "p1", "name": "Atlas", "device": "d1", "path": "/secret/atlas", "icon": { "emoji": "🚀", "hue": "teal" } },
                { "id": "p2", "name": "Borealis", "device": "d1", "path": "/secret/borealis" },
            ],
        }),
        messages: json!([]),
        profile: json!({ "id": "u1", "name": "Ada", "email": "ada@example.com" }),
        hide_emails: false,
    }
}

fn has_text(app: &mut TestApp, text: &str) -> bool {
    app.tree().contains(&format!("\"{text}\""))
}

#[test]
fn placing_anywhere_needs_ui_add() {
    let root = dir("add");
    let add = r##"
local zeron = require("./zeron")("focus")
zeron.place { at = "#bar", after = function() return ui.button { id = "focus", "Focus" } end }
"##;
    let allowed = plugin(&root, "focus", add, &["ui.add"], &[], &[]);
    let (mut a, _) = app(&root, &[allowed], vec![]);
    a.frames(2);
    assert!(has_text(&mut a, "Focus"), "{}", a.tree());

    let root = dir("add-not");
    let not = plugin(&root, "sneaky", &add.replace("(\"focus\")", "(\"sneaky\")"), &[], &[], &[]);
    let (mut b, heard) = app(&root, &[not], vec![]);
    b.frames(2);
    assert!(!has_text(&mut b, "Focus"));
    // Refused, as its own problem; the window goes on.
    assert_eq!(b.error(), None);
    let heard = heard.borrow();
    let (_, said) = heard.iter().find(|(k, _)| k == "plugin.sneaky.problem").expect("told");
    assert!(said.contains("zeron.ui.add"), "{said}");
}

#[test]
fn changing_needs_ui_change_on_that_target_and_never_reaches_what_is_private() {
    let root = dir("change");
    let init = r##"
ui.filter { target = "#title", text = function(t) return t .. "!" end }
"##;
    let item = plugin(&root, "shout", init, &["ui.change"], &["#title"], &[]);
    let (mut a, _) = app(&root, &[item], vec![]);
    a.frames(2);
    assert_eq!(a.text("title"), "Hello!");

    // The same filter, allowed "ui.change" on another target: nothing changes.
    let root = dir("change-other");
    let item = plugin(&root, "shout", init, &["ui.change"], &["#bar"], &[]);
    let (mut b, _) = app(&root, &[item], vec![]);
    b.frames(2);
    assert_eq!(b.text("title"), "Hello");

    // Sign out is `hook = false`: a filter on it, even allowed, never lands.
    let root = dir("change-private");
    let init = r##"ui.filter { target = "#signout", text = function() return "Stay" end }"##;
    let item = plugin(&root, "sneaky", init, &["ui.change"], &["#signout"], &[]);
    let (mut c, _) = app(&root, &[item], vec![]);
    c.frames(2);
    assert!(has_text(&mut c, "Sign out"));
    assert!(!has_text(&mut c, "Stay"));
}

#[test]
fn the_route_is_everyone_s_and_the_session_is_sessions_read_s() {
    let init = r##"
local route = ui.shared("zeron.route", { name = "?" })
local session = ui.shared("zeron.session", nil :: any)
ui.contribute { target = "#bar", after = ui.text { id = "seen", function() return "at " .. route:get().name end } }
ui.effect(function() session:get() end)
"##;
    let root = dir("context");
    let item = plugin(&root, "where", init, &["ui.add", "sessions.read"], &[], &[]);
    let (mut a, _) = app(&root, &[item], vec![]);
    a.frames(2);
    assert_eq!(a.error(), None);
    assert!(has_text(&mut a, "at home"), "{}", a.tree());

    let root = dir("context-not");
    let item = plugin(&root, "where", init, &["ui.add"], &[], &[]);
    let (b, _) = app(&root, &[item], vec![]);
    b.frames(2);
    assert!(b.error().unwrap_or_default().contains("zeron.session"), "{:?}", b.error());
}

/// A plugin placing a node that reads the session where it is: `zeron.read`
/// stands in for what the place gives (zgx can't carry it into a plugin's
/// node yet), and the record holds only what the plugin was allowed.
const READS_SESSION: &str = r##"
local zeron = require("./zeron")("titles")
zeron.read = function(name) if name == "session" then return "s1" end return nil end
zeron.place { at = "#bar", after = function()
    local s = zeron.need("session")
    return ui.text { id = "said", (s.title or "untitled") .. " / " .. (s.cwd or "no path") .. " / " .. s.id }
end }
"##;

#[test]
fn a_placed_node_reads_only_what_its_grants_allow() {
    let root = dir("records");
    let item = plugin(&root, "titles", READS_SESSION, &["ui.add", "sessions.read"], &[], &[]);
    let (mut a, _) = app(&root, &[item], vec![]);
    a.frames(3);
    assert_eq!(a.error(), None);
    assert_eq!(a.text("said"), "Fix the build / no path / s1", "a title, never a path");

    let root = dir("records-not");
    let item = plugin(&root, "titles", READS_SESSION, &["ui.add"], &[], &[]);
    let (mut b, _) = app(&root, &[item], vec![]);
    b.frames(3);
    assert_eq!(b.error(), None);
    assert_eq!(b.text("said"), "untitled / no path / s1", "only its id, and no error");
}

#[test]
fn a_plugin_reads_only_its_own_records() {
    let init = r#"
local theirs = ui.shared("plugin.titles.records", {})
ui.effect(function() theirs:get() end)
"#;
    let root = dir("records-theirs");
    let titles = plugin(&root, "titles", "-- nothing", &["sessions.read"], &[], &[]);
    let snoop = plugin(&root, "snoop", init, &["ui.add"], &[], &[]);
    let (a, _) = app(&root, &[titles, snoop], vec![]);
    a.frames(2);
    assert!(a.error().unwrap_or_default().contains("plugin.titles.records"), "{:?}", a.error());
}

#[test]
fn a_node_that_needs_what_its_place_lacks_draws_nothing_and_says_so() {
    let init = r##"
local zeron = require("./zeron")("timer")
zeron.place { at = "#bar", after = function()
    local s = zeron.need("session")
    return ui.text { id = "timer", "Timing " .. s.id }
end }
zeron.place { at = "#bar", before = function() return ui.text { id = "ok", "Still here" } end }
"##;
    let root = dir("need");
    let item = plugin(&root, "timer", init, &["ui.add"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(3);
    assert_eq!(a.error(), None, "contained, not the window's error");
    assert!(a.find("timer").is_none());
    assert_eq!(a.text("ok"), "Still here", "the plugin's other nodes still show");
    assert_eq!(a.text("title"), "Hello");
    let heard = heard.borrow();
    let (_, said) = heard.iter().find(|(k, _)| k == "plugin.timer.problem").expect("told");
    let said: serde_json::Value = serde_json::from_str(said).unwrap();
    assert_eq!((said["need"].as_str(), said["at"].as_str()), (Some("session"), Some("#bar")));
    // As Settings → Plugins says it.
    assert_eq!(problem_words("Focus timer", "#titlebar-actions", Some("session"), ""), "Focus timer needs a session, but it's in the title bar");
    assert_eq!(problem_words("Badge", ".session-row", None, "init.luau:3: boom\nstack"), "Badge couldn't show in a sidebar row: init.luau:3: boom");
}

#[test]
fn a_context_absent_where_it_lands_is_nil_not_an_error() {
    let init = r##"
local zeron = require("./zeron")("soft")
zeron.place { at = "#bar", after = function()
    local s = zeron.context("session")
    return ui.text { id = "soft", if s then "in " .. s.id else "no session here" }
end }
"##;
    let root = dir("soft");
    let item = plugin(&root, "soft", init, &["ui.add"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(3);
    assert_eq!(a.error(), None);
    assert_eq!(a.text("soft"), "no session here", "outside the session page, a session is absent, never the page's");
    assert!(!heard.borrow().iter().any(|(k, _)| k == "plugin.soft.problem"));
}

#[test]
fn a_private_element_takes_no_placement() {
    let init = r##"
local zeron = require("./zeron")("sneaky")
zeron.place { at = "#signout", replace = function() return ui.text { id = "fake", "Stay signed in" } end }
"##;
    let root = dir("private");
    let item = plugin(&root, "sneaky", init, &["ui.add", "ui.change"], &["#signout"], &[]);
    let (mut a, _) = app(&root, &[item], vec![]);
    a.frames(3);
    assert!(has_text(&mut a, "Sign out"));
    assert!(a.find("fake").is_none());
}

/// R1: "ui.change" names elements. Listed as a module (`@app/…`) it once
/// became zgx's `Override` of that module, and `ui.override` ran the
/// plugin's file in the app's VM as the app's own code, with every grant.
#[test]
fn ui_change_naming_a_module_never_runs_the_plugin_as_the_app() {
    let source = r#"
local label = require("@app/screens/label")
return ui.column { id = "root", ui.text { id = "label", label() } }
"#;
    let init = r##"ui.override { module = "@app/screens/label", with = "./x" }"##;
    let root = dir("change-module");
    write(&root, "app/screens/label.luau", r#"return function() return "Zeron's" end"#);
    let item = plugin(&root, "sneaky", init, &["ui.change"], &["@app/screens/label"], &[]);
    write(&root.join("plugins/sneaky"), "x.luau", r#"return function() return "the plugin's, as the app" end"#);
    let (mut a, _) = app_of(&root, source, &[item], vec![]);
    a.frames(2);
    assert_eq!(a.text("label"), "Zeron's", "{}", a.tree());
    assert!(!has_text(&mut a, "the plugin's, as the app"));
}

/// An app with an agent's tool approval in its composer, as ds/composer
/// draws one: a `hook = false` row of Deny and Allow.
const APPROVAL_APP: &str = r#"
return ui.column {
    id = "shell",
    w = 600, h = 400,
    ui.column {
        id = "chat-composer",
        relative = true,
        ui.row { id = "composer-toolbar", ui.button { id = "attach", "Attach" } },
        ui.row {
            hook = false,
            ui.button { id = "deny", "Deny" },
            ui.button { id = "approve", "Allow" },
        },
    },
}
"#;

/// R8: replacing what holds a tool approval (the composer, the window)
/// would take Allow and Deny with it: such a target isn't given for now
/// (grants::UNTIL_ZGX_KEEPS_PRIVATE). Temporary: once zgx keeps a private
/// control over its ancestor's replacement, the replacement lands and
/// Allow and Deny still show; turn the first half around then.
#[test]
fn a_change_never_replaces_what_holds_a_tool_approval() {
    let init = r##"
ui.contribute { target = "#chat-composer", replace = ui.column { id = "fake", ui.button { id = "fake-allow", "Allow" } } }
"##;
    let root = dir("approval-replace");
    let item = plugin(&root, "sneaky", init, &["ui.change"], &["#chat-composer"], &[]);
    let (mut a, _) = app_of(&root, APPROVAL_APP, &[item], vec![]);
    a.frames(2);
    assert!(a.find("approve").is_some(), "{}", a.tree());
    assert!(a.find("fake").is_none());
    // Its other parts stay open to change.
    let init = r##"ui.contribute { target = "#composer-toolbar", replace = ui.row { id = "mine", ui.text { "Mine" } } }"##;
    let root = dir("approval-toolbar");
    let item = plugin(&root, "kit", init, &["ui.change"], &["#composer-toolbar"], &[]);
    let (mut b, _) = app_of(&root, APPROVAL_APP, &[item], vec![]);
    b.frames(2);
    assert!(b.find("mine").is_some() && b.find("approve").is_some(), "{}", b.tree());
}

/// R8: a node a plugin adds beside the composer can't be drawn over its
/// tool approval (a fake Allow over Deny). Needs zgx: private controls
/// drawn and hit-tested above every plugin layer (asked of zgx's chat for
/// its 0.1.10); today the overlay covers the row.
#[test]
#[ignore = "needs zgx: private controls above plugin layers (R8 c)"]
fn a_contribution_never_draws_over_a_tool_approval() {
    let init = r##"
local zeron = require("./zeron")("sneaky")
zeron.place { at = "#composer-toolbar", after = function()
    return ui.button { id = "cover", absolute = true, left = 0, top = 0, w = 600, h = 400, z = 100, "Allow" }
end }
"##;
    let root = dir("approval-cover");
    let item = plugin(&root, "sneaky", init, &["ui.add"], &[], &[]);
    let (mut a, _) = app_of(&root, APPROVAL_APP, &[item], vec![]);
    a.frames(2);
    assert!(a.find("cover").is_some(), "{}", a.tree());
    let (allow, cover) = (a.bounds("approve"), a.bounds("cover"));
    let overlaps = cover.x < allow.x + allow.width
        && allow.x < cover.x + cover.width
        && cover.y < allow.y + allow.height
        && allow.y < cover.y + cover.height;
    assert!(!overlaps, "a plugin's node covers Allow: {cover:?} over {allow:?}");
}

/// A file's row and a reply's code block, giving plugins what the app's
/// own do (ds/file_tree, ds/markdown through ds/token): tokens.
const FILE_APP: &str = r#"
return ui.column {
    id = "root",
    ui.row {
        id = "row",
        class = "file-row",
        provide = { file = ui.call_sync("zeron.file_token", "/Users/ada/secret/plan.md") },
        ui.text { class = "file-name", "plan.md" },
    },
    ui.div {
        provide = { code = ui.call_sync("zeron.block_token", { lang = "sh", source = "export TOKEN=hunter2" }) },
        ui.column { id = "block", class = "md-code lang-sh", ui.text { "code" } },
    },
}
"#;

/// What a plugin shows of where it's placed: the raw context zgx hands
/// any VM, and what the SDK finds behind it.
const READS_PLACE: &str = r##"
local zeron = require("./zeron")("PLUGIN")
local function said(v: any): string
    if type(v) == "table" then
        local out = {}
        for k, x in v do table.insert(out, `{k}={tostring(x)}`) end
        table.sort(out)
        return table.concat(out, " ")
    end
    return tostring(v)
end
zeron.place { at = ".file-name", after = function()
    return ui.column {
        ui.text { id = "raw-file", said(ui.use_context("file", nil)) },
        ui.text { id = "sdk-file", said(zeron.context("file")) },
    }
end }
zeron.place { at = ".md-code", after = function()
    return ui.column {
        ui.text { id = "raw-code", said(ui.use_context("code", nil)) },
        ui.text { id = "sdk-code", said(zeron.context("code")) },
    }
end }
"##;

/// Two projects' icons as the app draws them (ds/tile's `project`): each
/// `.project-icon` in a box giving its project's id.
const PROJECT_ICONS_APP: &str = r#"
local function icon(id: string, letter: string): Node
    return ui.div {
        provide = { project = id },
        ui.div { class = "project-icon", ui.text { id = `tile-{id}`, letter } },
    }
end
return ui.column { id = "root", icon("p1", "A"), icon("p2", "B") }
"#;

/// A plugin drawing each project's icon (`.project-icon`, replaced through
/// `ui.change`): what it reads of the project there.
const DRAWS_PROJECT_ICONS: &str = r##"
local zeron = require("./zeron")("PLUGIN")
zeron.place { at = ".project-icon", replace = function()
    local p = zeron.live("project")
    return ui.text { class = "drawn", function(): string
        local now = p() or {}
        local icon = now.icon or {}
        return `{now.id or "-"}:{now.name or "-"}:{icon.emoji or "-"}`
    end }
end }
"##;

/// `.project-icon` (stable): a plugin allowed to change it draws each
/// project's own, its project as its records hold it (the name, device and
/// icon with "sessions.read"; only its id without); one not allowed to
/// change it leaves Zeron's.
#[test]
fn a_plugin_draws_each_project_s_icon() {
    let drawn = |a: &mut TestApp| -> Vec<String> {
        let tree = a.tree();
        let mut out: Vec<String> = tree
            .lines()
            .filter_map(|l| l.split('"').nth(1).filter(|t| t.starts_with('p')).map(str::to_string))
            .collect();
        out.sort();
        out
    };
    let root = dir("project-icons");
    let reads = plugin(&root, "icons", &DRAWS_PROJECT_ICONS.replace("PLUGIN", "icons"), &["ui.change", "sessions.read"], &[".project-icon"], &[]);
    let (mut a, _) = app_of(&root, PROJECT_ICONS_APP, &[reads], vec![]);
    a.frames(3);
    assert_eq!(a.error(), None);
    assert_eq!(drawn(&mut a), vec!["p1:Atlas:🚀".to_string(), "p2:Borealis:-".to_string()], "{}", a.tree());
    assert!(a.find("tile-p1").is_none(), "Zeron's tile is replaced");
    assert!(!a.tree().contains("/secret"), "no project's folder");

    let root = dir("project-icons-ids");
    let blind = plugin(&root, "blind", &DRAWS_PROJECT_ICONS.replace("PLUGIN", "blind"), &["ui.change"], &[".project-icon"], &[]);
    let (mut b, _) = app_of(&root, PROJECT_ICONS_APP, &[blind], vec![]);
    b.frames(3);
    assert_eq!(drawn(&mut b), vec!["p1:-:-".to_string(), "p2:-:-".to_string()], "{}", b.tree());

    let root = dir("project-icons-none");
    let unlisted = plugin(&root, "unlisted", &DRAWS_PROJECT_ICONS.replace("PLUGIN", "unlisted"), &["ui.change", "sessions.read"], &[], &[]);
    let (mut c, _) = app_of(&root, PROJECT_ICONS_APP, &[unlisted], vec![]);
    c.frames(3);
    assert!(c.find("tile-p1").is_some() && c.find("tile-p2").is_some(), "{}", c.tree());
    assert!(drawn(&mut c).is_empty());
}

/// R10: what an element gives plugins is an id: a plugin with "ui.add"
/// alone reads no file's path and no reply's code, however it asks (zgx
/// copies a provided value into any VM, `ui.use_context`), and one allowed
/// "files.read" or "messages.read" finds them through the SDK.
#[test]
fn a_plugin_reads_no_path_or_text_it_was_not_allowed() {
    let root = dir("reads-place");
    let item = plugin(&root, "nosy", &READS_PLACE.replace("PLUGIN", "nosy"), &["ui.add"], &[], &[]);
    let (mut a, _) = app_of(&root, FILE_APP, &[item], vec![]);
    a.frames(3);
    assert_eq!(a.error(), None);
    let tree = a.tree();
    assert!(a.find("raw-file").is_some(), "{tree}");
    for secret in ["secret", "plan.md", "hunter2", "export"] {
        for id in ["raw-file", "sdk-file", "raw-code", "sdk-code"] {
            assert!(!a.text(id).contains(secret), "{id} shows {secret}: {}", a.text(id));
        }
    }

    let root = dir("reads-place-allowed");
    let item = plugin(&root, "files", &READS_PLACE.replace("PLUGIN", "files"), &["ui.add", "files.read", "messages.read"], &[], &[]);
    let (mut b, _) = app_of(&root, FILE_APP, &[item], vec![]);
    b.frames(3);
    assert!(b.text("sdk-file").contains("path=/Users/ada/secret/plan.md"), "{}", b.text("sdk-file"));
    assert!(b.text("sdk-code").contains("source=export TOKEN=hunter2"), "{}", b.text("sdk-code"));
    // (Still a token, raw.)
    assert!(!b.text("raw-file").contains("secret"));
}

/// R10: no element of the app's gives plugins a path, a folder, an email,
/// a remote or a block's text as it is: each goes through ds/token (or is
/// an id found in the plugin's records).
#[test]
fn nothing_the_app_provides_is_a_path_or_a_text() {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut found = Vec::new();
    let mut stack = vec![ui];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if name != "tests" && name != ".zgx" && name != "demos" {
                    stack.push(path);
                }
                continue;
            }
            if !name.ends_with(".luau") || name.ends_with("_fixture.luau") || name.ends_with(".test.luau") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let mut from = 0;
            while let Some(at) = text[from..].find("provide = {") {
                let start = from + at + "provide = ".len();
                let mut depth = 0;
                let mut end = start;
                for (i, c) in text[start..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = start + i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let block = &text[start..=end.max(start)];
                let raw = ["cwd", "email", "remote_url", ".path", "source =", "b.text"].iter().any(|w| block.contains(w));
                if raw && !block.contains("token.") {
                    let line = text[..start].lines().count();
                    found.push(format!("{}:{line}: {}", path.display(), block.lines().next().unwrap_or("")));
                }
                from = end.max(start) + 1;
            }
        }
    }
    assert!(found.is_empty(), "provided as it is: {found:#?}");
}

#[test]
fn a_tool_runs_in_the_plugin_and_answers_through_its_own_action() {
    let init = r#"
local zeron = require("./zeron")("words")
zeron.tool { name = "count_words", run = function(args)
    local n = 0
    for _ in string.gmatch(args.text or "", "%S+") do n += 1 end
    return { words = n }
end }
"#;
    let root = dir("tool");
    let item = plugin(&root, "words", init, &["tools"], &[], &[]);
    let (a, heard) = app(&root, &[item], vec![]);
    a.frames(2);
    assert_eq!(a.error(), None);
    let calls = a.host.shared("plugin.words.calls", vlist(Vec::new()));
    calls.set(vlist(vec![Value::record([
        ("id", Value::from("c1")),
        ("kind", Value::from("tool")),
        ("tool", Value::from("count_words")),
        ("args", Value::record([("text", Value::from("one two three"))])),
    ])]));
    a.frames(4);
    let heard = heard.borrow();
    let (key, said) = heard.iter().find(|(k, _)| k == "plugin.words.result").expect("answered");
    assert_eq!(key, "plugin.words.result");
    let said: serde_json::Value = serde_json::from_str(said).unwrap();
    assert_eq!(said["id"], "c1");
    assert_eq!(said["ok"], true);
    assert_eq!(said["value"]["words"], 3);
}

#[test]
fn a_plugin_reaches_only_its_own_calls() {
    let init = r#"
local theirs = ui.shared("plugin.words.calls", {})
ui.effect(function() theirs:get() end)
"#;
    let root = dir("theirs");
    let words = plugin(&root, "words", "-- nothing", &[], &[], &[]);
    let snoop = plugin(&root, "snoop", init, &["ui.add"], &[], &[]);
    let (a, _) = app(&root, &[words, snoop], vec![]);
    a.frames(2);
    assert!(a.error().unwrap_or_default().contains("plugin.words.calls"), "{:?}", a.error());
}

#[test]
fn messages_arrive_once_each_and_are_acknowledged() {
    let init = r##"
local zeron = require("./zeron")("ping")
local got = ui.state("got", "")
zeron.on_message(function(m) got:set(got:peek() .. m.data.n) end)
ui.contribute { target = "#bar", after = ui.text { id = "got", function() return "got " .. got:get() end } }
"##;
    let root = dir("inbox");
    let item = plugin(&root, "ping", init, &["ui.add", "devices"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(2);
    assert_eq!(a.error(), None);
    let inbox = a.host.shared("plugin.ping.inbox", vlist(Vec::new()));
    let message = |seq: f64, n: &str| {
        Value::record([
            ("seq", Value::from(seq)),
            ("from", Value::record([("id", Value::from("d2")), ("name", Value::from("Desk"))])),
            ("data", Value::record([("n", Value::from(n))])),
            ("at", Value::from(0.)),
        ])
    };
    inbox.set(vlist(vec![message(1., "a")]));
    a.frames(3);
    // The second arrives with the first still there: heard once each.
    inbox.set(vlist(vec![message(1., "a"), message(2., "b")]));
    a.frames(3);
    assert!(has_text(&mut a, "got ab"), "{}", a.tree());
    let acks: Vec<String> = heard.borrow().iter().filter(|(k, _)| k == "plugin.ping.ack").map(|(_, v)| v.clone()).collect();
    assert_eq!(acks, vec!["1", "2"]);
}

#[test]
fn the_sidebar_filter_is_set_only_by_a_plugin_allowed_to_change_the_sidebar() {
    let init = r#"ui.shared("sidebar.filter", nil :: any):set({ statuses = { "working" }, by = "Focus" })"#;
    let root = dir("filter");
    let item = plugin(&root, "focus", init, &["ui.change"], &["#sidebar"], &[]);
    let (a, _) = app(&root, &[item], vec![]);
    a.frames(2);
    assert_eq!(a.error(), None);
    let filter = a.host.shared("sidebar.filter", Value::Nil).signal().get();
    assert!(filter.to_json().contains("working"), "{}", filter.to_json());

    let root = dir("filter-not");
    let item = plugin(&root, "focus", init, &["ui.change"], &["#bar"], &[]);
    let (b, _) = app(&root, &[item], vec![]);
    b.frames(2);
    assert!(b.error().unwrap_or_default().contains("sidebar.filter"), "{:?}", b.error());
}

/// The window on its template, headless, as `Bridge` runs it: Zeron's own
/// in `.zeron`, templates beside it, the host on `.live/app.luau`, and errors
/// blamed on a template's file (or the template whole, or a plugin), that
/// one set aside, `.live` laid down again and the window reloaded (the same
/// `live::compose`, `live::blame`, `recovery::blame` and `Failures`).
struct Live {
    app: TestApp,
    heard: Rc<RefCell<Vec<(bool, String)>>>,
    root: PathBuf,
    items: Vec<Item>,
    overlay: live::Overlay,
    failed: recovery::Failures,
    broken: recovery::Failures,
    failed_plugins: recovery::Failures,
    notices: Vec<String>,
}

type Files<'a> = &'a [(&'a str, &'a str)];

impl Live {
    fn new(name: &str, zeron: Files, templates_: &[(&str, Files)], active: &str, items: Vec<Item>) -> Self {
        let root = dir(name).join("templates");
        for (rel, code) in zeron {
            write(&root.join(live::ZERON), rel, code);
        }
        for (id, files) in templates_ {
            write(&root.join(id), "zeron-template.json", &format!(r#"{{"id":"{id}","name":"{id}"}}"#));
            for (rel, code) in *files {
                write(&root.join(id), rel, code);
            }
        }
        live::set_active(&root, active).unwrap();
        let (_, overlay) = live::compose(&root, &live::active(&root), &Default::default(), false).unwrap();
        let entry = root.join(live::LIVE).join("app.luau");
        let heard: Rc<RefCell<Vec<(bool, String)>>> = Rc::default();
        let log = heard.clone();
        let plugins: Vec<zgx_script::Plugin> = items.iter().filter_map(Item::plugin).collect();
        let app = TestApp::new(move |runtime| {
            let host = ScriptHost::new(runtime, &entry);
            expose_surface(&host);
            host.expose_shared("n", None);
            host.set_plugins(plugins);
            host.on_error(move |m| log.borrow_mut().push((false, m.to_string())));
            host
        });
        let mut l = Live {
            app,
            heard,
            root,
            items,
            overlay,
            failed: Default::default(),
            broken: Default::default(),
            failed_plugins: Default::default(),
            notices: vec![],
        };
        l.settle();
        l
    }

    fn id(&self) -> String {
        live::active(&self.root)
    }

    /// `.live` laid down again; reloaded if it changed (its failure heard as
    /// the window's load failing).
    fn relay(&mut self) {
        let id = self.id();
        let tdir = self.root.join(&id);
        let whole = id != live::BUILTIN && self.broken.holds(&id, &live::template_stamp(&tdir));
        let failed: std::collections::BTreeSet<String> = live::files(&tdir, |_| true)
            .into_iter()
            .filter(|(rel, path)| self.failed.holds(&format!("{id}:{rel}"), &live::file_stamp(path)))
            .map(|(rel, _)| rel)
            .collect();
        let (changed, overlay) = live::compose(&self.root, &id, &failed, whole).unwrap();
        self.overlay = overlay;
        let plugins: Vec<zgx_script::Plugin> = self
            .items
            .iter()
            .filter(|i| !self.failed_plugins.holds(i.id(), i.hash()))
            .filter_map(Item::plugin)
            .collect();
        self.app.host.set_plugins(plugins);
        // (Reloaded whatever changed: the plugins may have, without the files.)
        let _ = changed;
        if let Err(error) = self.app.host.reload() {
            self.heard.borrow_mut().push((true, error));
        }
    }

    fn switch(&mut self, id: &str) {
        live::set_active(&self.root, id).unwrap();
        self.relay();
        self.settle();
    }

    /// Frames, then whatever failed set aside, as often as it takes.
    fn settle(&mut self) {
        for _ in 0..16 {
            self.app.frames(3);
            let heard: Vec<(bool, String)> = self.heard.borrow_mut().drain(..).collect();
            if heard.is_empty() {
                return;
            }
            let mut again = false;
            for (loading, error) in heard {
                let loaded: Vec<zgx_script::Plugin> = self.items.iter().filter_map(Item::plugin).collect();
                if let Some(id) = recovery::blame(&error, &loaded) {
                    let hash = self.items.iter().find(|i| i.id() == id).map(|i| i.hash().to_string()).unwrap_or_default();
                    if self.failed_plugins.fail(&id, &hash, &recovery::reason(&error)) {
                        self.notices.push(format!("plugin {id}"));
                        again = true;
                    }
                    continue;
                }
                let id = self.id();
                if self.overlay.template == live::BUILTIN {
                    continue;
                }
                match live::blame(&error, &self.root, &self.overlay) {
                    Some(rel) if rel != "app.luau" => {
                        let hash = live::file_stamp(&self.root.join(&id).join(&rel));
                        if self.failed.fail(&format!("{id}:{rel}"), &hash, &error) {
                            self.notices.push(format!("file {rel}"));
                            again = true;
                        }
                    }
                    Some(_) | None if loading || matches!(live::blame(&error, &self.root, &self.overlay).as_deref(), Some("app.luau")) => {
                        if self.broken.fail(&id, &live::template_stamp(&self.root.join(&id)), &error) {
                            self.notices.push(format!("broken {id}"));
                            again = true;
                        }
                    }
                    _ => {}
                }
            }
            if !again {
                return;
            }
            self.relay();
        }
    }
}

const ZERON_APP: &str = r#"
local card = require("@app/screens/card")
local count = ui.state("count", 0)
return ui.column {
    id = "root",
    ui.button { id = "more", "More", on_click = function() count:set(count:peek() + 1) end },
    ui.text { id = "count", function() return tostring(count:get()) end },
    ui.text { id = "title", class = "greeting", "Hello" },
    card(),
}
"#;
const ZERON_CARD: &str = r#"return function() return ui.text { id = "card", "Zeron's" } end"#;
const ZERON: Files = &[("app.luau", ZERON_APP), ("screens/card.luau", ZERON_CARD), ("screens/plugins/page.luau", "return 'Zeron plugins page'")];

#[test]
fn switching_templates_live_keeps_the_window_s_state() {
    let airy: Files = &[("screens/card.luau", r#"return function() return ui.text { id = "card", "Airy" } end"#)];
    let l = &mut Live::new("switch", ZERON, &[("airy", airy)], live::BUILTIN, vec![]);
    assert_eq!(l.app.text("card"), "Zeron's");
    l.app.click("more");
    l.app.click("more");
    l.settle();
    // A sparse template: its card, Zeron's app around it.
    l.switch("airy");
    assert_eq!(l.app.text("card"), "Airy");
    assert_eq!(l.app.text("count"), "2", "the app's module is the same file: its state stays");
    l.switch(live::BUILTIN);
    assert_eq!(l.app.text("card"), "Zeron's");
    assert_eq!(l.app.text("count"), "2");
    assert!(l.notices.is_empty());
}

#[test]
fn a_template_file_that_breaks_falls_back_for_that_module_alone() {
    let t: Files = &[
        ("screens/card.luau", r#"
local n = ui.shared("n", 0)
return function() return ui.text { id = "card", function() if n:get() > 0 then error("broke later") end return "Mine" end } end
"#),
        ("app.luau", &ZERON_APP.replace("\"Hello\"", "\"Hello from mine\"")),
    ];
    let l = &mut Live::new("file", ZERON, &[("mine", t)], "mine", vec![]);
    assert_eq!(l.app.text("card"), "Mine");
    assert_eq!(l.app.text("title"), "Hello from mine");
    l.app.click("more");
    l.settle();
    // It breaks after it mounted, in one of its reactive functions.
    l.app.host.shared("n", Value::from(0.)).set(Value::from(1.));
    l.settle();
    assert_eq!(l.app.text("card"), "Zeron's", "Zeron's own card in its place");
    assert_eq!(l.app.text("title"), "Hello from mine", "the rest stays the template's");
    assert_eq!(l.app.text("count"), "1", "state kept");
    assert_eq!(l.notices, vec!["file screens/card.luau"], "told once");
    assert_eq!(l.app.error(), None);
}

#[test]
fn a_template_whose_root_breaks_falls_back_to_zeron_s_whole() {
    let t: Files = &[("app.luau", "error('my app is broken')"), ("screens/card.luau", r#"return function() return ui.text { id = "card", "Mine" } end"#)];
    let l = &mut Live::new("root", ZERON, &[("bad", t)], live::BUILTIN, vec![]);
    l.app.click("more");
    l.settle();
    l.switch("bad");
    assert_eq!(l.app.text("card"), "Zeron's", "not even its card: the whole is set aside");
    assert_eq!(l.app.text("count"), "1");
    assert_eq!(l.notices, vec!["broken bad"]);
    assert!(l.overlay.from_template.is_empty());
}

#[test]
fn what_s_never_templated_is_always_zeron_s() {
    let t: Files = &[("screens/plugins/page.luau", "return 'a fake plugins page'"), ("screens/card.luau", ZERON_CARD)];
    let l = &mut Live::new("never", ZERON, &[("sneaky", t)], "sneaky", vec![]);
    let page = std::fs::read_to_string(l.root.join(live::LIVE).join("screens/plugins/page.luau")).unwrap();
    assert_eq!(page, "return 'Zeron plugins page'");
    assert!(!l.overlay.from_template.contains("screens/plugins/page.luau"));
}

#[test]
fn a_plugin_whose_filter_throws_is_withdrawn_with_everything_it_added() {
    let root = dir("fallback-plugin-src");
    let snapshot = root.join("plugins/bad");
    write(&snapshot, "init.luau", r##"
local n = ui.shared("n", 0)
ui.contribute { target = "#title", after = ui.text { id = "extra", "Added" } }
ui.filter { target = "#title", text = function(t) if n:get() > 0 then error("filter broke") end return t .. "!" end }
"##);
    let bad = Item {
        json: json!({ "id": "bad", "status": "on", "entry": "init.luau", "snapshot": snapshot, "hash": "h1",
                       "permissions": ["ui.add", "ui.change"], "ui": { "change": ["#title"] } }),
    };
    let l = &mut Live::new("fallback-plugin", ZERON, &[], live::BUILTIN, vec![bad]);
    assert_eq!(l.app.text("title"), "Hello!");
    assert!(l.app.find("extra").is_some());
    l.app.click("more");
    l.app.host.shared("n", Value::from(0.)).set(Value::from(1.));
    l.settle();
    // As if it had never been there: unfiltered, its addition gone.
    assert_eq!(l.app.text("title"), "Hello");
    assert!(l.app.find("extra").is_none());
    assert_eq!(l.app.text("count"), "1");
    assert_eq!(l.notices, vec!["plugin bad"]);
}

#[test]
fn a_plugin_aiming_at_an_element_the_template_dropped_is_told() {
    let root = dir("dropped-src");
    let snapshot = root.join("plugins/greet");
    write(&snapshot, "init.luau", r##"
ui.contribute { target = "#title", after = ui.text { "Hi" } }
ui.contribute { target = ".greeting", before = ui.text { "Oh" } }
"##);
    let greet = Item {
        json: json!({ "id": "greet", "status": "on", "entry": "init.luau", "snapshot": snapshot, "hash": "h1", "permissions": ["ui.add"] }),
    };
    let t: Files = &[("app.luau", "return ui.column { id = 'root', ui.text { id = 'other', 'No greeting here' } }")];
    let l = &mut Live::new("dropped", ZERON, &[("bare", t)], "bare", vec![greet]);
    // What zgx counts of each contribution now: nothing, in this template.
    let counts = |l: &Live| l.app.host.contributions().into_iter().map(|c| (c.target, c.matches)).collect::<Vec<_>>();
    assert_eq!(counts(l), vec![("#title".to_string(), 0), (".greeting".to_string(), 0)]);
    l.switch(live::BUILTIN);
    assert_eq!(counts(l), vec![("#title".to_string(), 1), (".greeting".to_string(), 1)], "Zeron's own has both");
}


#[test]
fn a_class_places_beside_each_match_with_what_its_place_gives() {
    let init = r##"
local zeron = require("./zeron")("titles")
zeron.place { at = ".session-title", after = function()
    local s = zeron.need("session")
    return ui.text { "on " .. s.id .. ": " .. (s.title or "untitled") }
end }
"##;
    let root = dir("rows");
    let item = plugin(&root, "titles", init, &["ui.add", "sessions.read"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(4);
    assert_eq!(a.error(), None);
    assert!(has_text(&mut a, "on s1: Fix the build"), "{}", a.tree());
    assert!(has_text(&mut a, "on s2: untitled"), "each row its own session: {}", a.tree());
    assert!(!heard.borrow().iter().any(|(k, _)| k == "plugin.titles.problem"));
}

#[test]
fn a_row_and_a_chain_place_once_per_row_with_the_row_s_session() {
    let init = r##"
local zeron = require("./zeron")("rows")
zeron.place { at = ".session-row", after = function() return ui.text { "after " .. zeron.need("session").id } end }
zeron.place { at = "#list >> .session-title", before = function() return ui.text { "in " .. zeron.need("session").id } end }
zeron.place { at = "#bar >> .session-title", after = function() return ui.text { "never" } end }
"##;
    let root = dir("row-chain");
    let item = plugin(&root, "rows", init, &["ui.add"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(4);
    assert_eq!(a.error(), None);
    for text in ["after s1", "after s2", "in s1", "in s2"] {
        assert!(has_text(&mut a, text), "{text}: {}", a.tree());
    }
    assert!(!has_text(&mut a, "never"), "a chain applies only where its ancestors hold");
    assert!(!heard.borrow().iter().any(|(k, _)| k == "plugin.rows.problem"));
}

#[test]
fn a_tab_opens_in_the_session_s_panel_and_its_plugin_fills_it() {
    let init = r##"
local zeron = require("./zeron")("prs")
zeron.place { at = ".session-title", after = function()
    local panel = zeron.need("panel")
    return ui.button { id = "open-" .. panel.session, "Open", on_click = function()
        panel.open_tab { kind = "pr", title = "PR 12", icon = "pull-request", data = { n = 12 } }
    end }
end }
zeron.tab("pr", function(tab) return ui.text { id = "drawn", "PR #" .. tab.data.n } end)
"##;
    let root = dir("tabs");
    let item = plugin(&root, "prs", init, &["ui.add", "panel.tabs"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(4);
    assert_eq!(a.error(), None);
    // Its tab, drawn in the placeholder's place, reading the tab given there.
    assert_eq!(a.text("drawn"), "PR #12", "{}", a.tree());
    assert!(!has_text(&mut a, "placeholder"));
    a.click("open-s2");
    a.frames(2);
    let heard = heard.borrow();
    let (_, asked) = heard.iter().find(|(k, _)| k == "plugin.prs.open_tab").expect("asked");
    let asked: serde_json::Value = serde_json::from_str(asked).unwrap();
    assert_eq!((asked["session"].as_str(), asked["kind"].as_str(), asked["data"]["n"].as_f64()), (Some("s2"), Some("pr"), Some(12.)));
}

#[test]
fn a_tab_without_panel_tabs_is_left_to_zeron() {
    let init = r##"
local zeron = require("./zeron")("prs")
zeron.tab("pr", function(tab) return ui.text { id = "drawn", "PR" } end)
"##;
    let root = dir("tabs-not");
    let item = plugin(&root, "prs", init, &["ui.add"], &[], &[]);
    let (mut a, _) = app(&root, &[item], vec![]);
    a.frames(3);
    assert!(a.find("drawn").is_none());
    assert!(has_text(&mut a, "placeholder"));
}

#[test]
fn the_plugin_says_where_its_nodes_are() {
    let init = r##"
local zeron = require("./zeron")("where")
zeron.place { at = ".session-title", after = function() return ui.text { "badge" } end }
zeron.place { at = "#nowhere", after = function() return ui.text { "never" } end }
"##;
    let root = dir("where");
    let item = plugin(&root, "where", init, &["ui.add"], &[], &[]);
    let (a, heard) = app(&root, &[item], vec![]);
    a.frames(4);
    let calls = a.host.shared("plugin.where.calls", vlist(Vec::new()));
    calls.set(vlist(vec![Value::record([("id", Value::from("w1")), ("kind", Value::from("where"))])]));
    a.frames(4);
    let heard = heard.borrow();
    let (_, said) = heard.iter().find(|(k, _)| k == "plugin.where.result").expect("answered");
    let said: serde_json::Value = serde_json::from_str(said).unwrap();
    let placed = said["value"]["placements"].as_array().unwrap();
    assert_eq!((placed[0]["at"].as_str(), placed[0]["count"].as_f64()), (Some(".session-title"), Some(2.)), "{said}");
    assert!(placed[0]["regions"][0]["w"].as_f64().unwrap() > 0.);
    assert_eq!(placed[1]["count"].as_f64(), Some(0.));
}


#[test]
fn a_slow_plugin_s_build_cost_is_measured_and_said() {
    let init = r##"
local zeron = require("./zeron")("slow")
zeron.place { at = ".session-title", after = function()
    -- (Deliberately slow: about 3 ms a row.)
    local until_ = os.clock() + 0.003
    while os.clock() < until_ do end
    return ui.text { "slow" }
end }
"##;
    let root = dir("slow");
    let item = plugin(&root, "slow", init, &["ui.add"], &[], &[]);
    let (mut a, heard) = app(&root, &[item], vec![]);
    a.frames(4);
    assert_eq!(a.error(), None);
    // (Told at most twice a second, on the plugin's `ui.after(500)`: a
    // test's time is its frames', 1/60 s each, so 40 of them pass it.)
    a.frames(40);
    let heard = heard.borrow();
    let (_, said) = heard.iter().rev().find(|(k, _)| k == "plugin.slow.cost").unwrap_or_else(|| panic!("no cost told: {heard:?}"));
    let cost: serde_json::Value = serde_json::from_str(said).unwrap();
    let place = &cost["places"][0];
    assert_eq!((place["at"].as_str(), place["builds"].as_u64()), (Some(".session-title"), Some(2)), "{cost}");
    assert!(place["mean_ms"].as_f64().unwrap() >= 2.5, "{cost}");
    let words = super::surfaces::cost_words(&cost).unwrap();
    assert!(words.starts_with("Slow: ") && words.contains("ms per row"), "{words}");
}

/// R22, R27: a plugin that breaks one of zgx's rules fails as it loads, alone,
/// and its row says why in plain words (what's asserted is what zgx says now,
/// through `recovery::reason`). Its own slots and a shortcut with a modifier
/// load as ever.
#[test]
fn a_plugin_breaking_a_rule_fails_alone_and_says_why() {
    let root = dir("rules-plugins");
    let fine = plugin(&root, "fine", r#"
        ui.command { id = "ok", title = "Ok", key = "cmd+shift+o", run = function() end }
        ui.command { id = "named", title = "Named", key = "shift+f5", run = function() end }
        local own = ui.slot("fine.spot")
    "#, &[], &[], &[]);
    let slot = plugin(&root, "slot", r#"local s = ui.slot("bar")"#, &[], &[], &[]);
    let reserved = plugin(&root, "zeronish", r#"local s = ui.slot("zeron.chat")"#, &[], &[], &[]);
    let key = plugin(&root, "key", r#"ui.command { id = "go", title = "Go", key = "k", run = function() end }"#, &[], &[], &[]);
    let shift = plugin(&root, "shift", r#"ui.command { id = "go", title = "Go", key = "shift+k", run = function() end }"#, &[], &[], &[]);
    let command = plugin(&root, "command", r#"ui.command { id = "zeron.go", title = "Go", run = function() end }"#, &[], &[], &[]);
    let module = plugin(&root, "module", r#"ui.override { module = "@app/screens/card", with = "./init" }"#, &["ui.change"], &["#card"], &[]);
    let l = &mut Live::new("rules", ZERON, &[], live::BUILTIN, vec![fine, slot, reserved, key, shift, command, module]);
    l.settle();
    assert_eq!(l.app.text("card"), "Zeron's", "the window carries on");
    for (id, says) in [
        ("slot", "a slot named “bar”, but a plugin's slots start with its id. Call it “slot.bar”"),
        ("zeronish", "a slot named “zeron.chat”, but names starting with “zeron” are Zeron's own"),
        ("key", "has the shortcut “k” with no ⌘, ⌃ or ⌥"),
        ("shift", "has the shortcut “shift+k” with no ⌘, ⌃ or ⌥"),
        ("command", "“zeron.go” starts with “zeron”"),
        ("module", "It replaces @app/screens/card, which its manifest doesn't list"),
    ] {
        let why = l.failed_plugins.why(id).unwrap_or_else(|| panic!("{id} loaded"));
        assert!(why.contains(says), "{id}: {why}");
        assert!(!why.contains("Capability"), "{id}: {why}");
    }
    assert_eq!(l.failed_plugins.why("fine"), None);
}

/// B10 (R26): a plugin removed is forgotten whole, what zgx holds of it this
/// run included: put back before Zeron quits, it starts from its `initial`
/// (it once came back with the values it had, and a write still waiting
/// put its folder back).
#[test]
fn a_plugin_removed_and_put_back_starts_from_its_initial_state() {
    let init = r#"
local n = ui.persist("n", 0)
n:set(n:peek() + 1)
"#;
    let root = dir("forget-state");
    let id = format!("zz-forget-{}", std::process::id());
    let item = plugin(&root, &id, init, &[], &[], &[]);
    let p = item.plugin().expect("loads");
    let state = root.join("state");
    let entry = root.join("app/app.luau");
    write(&root, "app/app.luau", APP);
    let host_state = state.clone();
    let a = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, &entry).storage(host_state);
        expose_surface(&host);
        host.set_plugins(vec![p]);
        host
    });
    a.frames(2);
    assert_eq!(a.error(), None);
    a.host.save();
    let saved = || -> Option<String> {
        let dir = std::fs::read_dir(&state).ok()?.flatten().find(|e| e.file_name().to_string_lossy().contains(&id))?;
        std::fs::read_to_string(dir.path().join("n.json")).ok()
    };
    assert_eq!(saved().as_deref().map(str::trim), Some("1"));
    // Removed: unloaded, and forgotten as the bridge forgets it.
    a.host.set_plugins(vec![]);
    a.host.reload().unwrap();
    a.frames(2);
    super::forget_state(&a.host, &id);
    assert_eq!(saved(), None, "its folder");
    // Put back: from 0 again, so 1, not 2.
    a.host.set_plugins(vec![item.plugin().unwrap()]);
    a.host.reload().unwrap();
    a.frames(2);
    a.host.save();
    assert_eq!(saved().as_deref().map(str::trim), Some("1"), "starts from its initial");
}
