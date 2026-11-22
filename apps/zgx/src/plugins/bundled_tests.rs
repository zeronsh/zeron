//! Zeron's own plugins shipped inside the app (plugins-builtin) are trusted
//! as the app's code with the app modules their manifest names
//! (`app_modules`): the replacement runs in the app's VM and requires
//! `@app/…` and the plugin's own files. Only a plugin the engine lists as a
//! built-in by Zeron from the very folder this window ships is: one you
//! installed with the same id is not, nor a list naming a folder elsewhere.
//! Off, it's gone with the next load, and the app's own module is back.

use std::path::{Path, PathBuf};

use serde_json::json;
use zgx_script::{Capability, ScriptHost, testing::TestApp};

use super::{Item, grants};

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-plugin-bundled-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The app: what its `@app/builtin/demo` gives, shown (its own is `false`).
const APP: &str = r#"
local demo = require("@app/builtin/demo")
return ui.column {
    id = "root",
    ui.text { id = "label", if type(demo) == "table" then demo.label() else "the app's own" },
}
"#;

/// A plugin folder `dir` as Orchestrators is laid out: its manifest naming
/// `@app/builtin/demo`, an init putting `./index` in its place, and an index
/// reaching the app (`@app/account/secret`, app code alone) and its own
/// files.
fn lay_out(dir: &Path) {
    write(dir, "zeron-plugin.json", &json!({
        "id": "demo", "name": "Demo", "version": "1.0.0", "entry": "init.luau",
        "switch": { "default": "off", "scope": "account" }, "app_modules": ["@app/builtin/demo"],
    }).to_string());
    write(dir, "init.luau", r#"ui.override { module = "@app/builtin/demo", with = "./index" }"#);
    write(dir, "index.luau", r#"
local secret = require("@app/account/secret")
local words = require("./app/words")
return { label = function() return words.mine .. " + " .. secret.word end }
"#);
    write(dir, "app/words.luau", r#"return { mine = "the plugin's" }"#);
}

/// An app at `root/app`, Zeron's own plugins at `root/plugins-builtin`
/// (`demo` among them), and the engine's word on `demo` as `listed`.
fn setup(name: &str) -> (PathBuf, PathBuf) {
    let root = dir(name);
    write(&root, "app/app.luau", APP);
    write(&root, "app/builtin/demo.luau", "return false");
    write(&root, "app/account/secret.luau", r#"return { word = "the app's secret" }"#);
    let builtin = root.join("plugins-builtin");
    lay_out(&builtin.join("demo"));
    // The engine's snapshot of it: what an ordinary plugin loads from. A
    // bundled one loads from the app's folder instead.
    write(&root, "snapshot/demo/init.luau", "-- the snapshot's");
    (root, builtin)
}

fn item(root: &Path, status: &str, builtin: bool, author: &str, dir: &Path) -> Item {
    Item {
        json: json!({
            "id": "demo", "name": "Demo", "status": status, "entry": "init.luau",
            "builtin": builtin, "author": { "kind": author }, "dir": dir,
            "snapshot": root.join("snapshot/demo"), "permissions": [],
            "ui": { "change": [], "replace": [] },
        }),
    }
}

fn mount(root: &Path, plugins: Vec<zgx_script::Plugin>) -> TestApp {
    let entry = root.join("app/app.luau");
    let a = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, &entry);
        host.set_plugins(plugins);
        host
    });
    a.frames(2);
    a
}

#[test]
fn a_bundled_built_in_replaces_the_app_modules_it_names_as_the_app() {
    let (root, builtin) = setup("granted");
    let item = item(&root, "on", true, "zeron", &builtin.join("demo"));
    let bundled = item.bundled(&builtin).expect("one of Zeron's own, from the app's folder");
    assert_eq!(bundled.modules, vec!["@app/builtin/demo".to_string()]);
    let plugin = item.plugin_with(&builtin).expect("on: it loads");
    // From the app's folder, not the engine's copy.
    assert!(plugin.entry.starts_with(builtin.canonicalize().unwrap()), "{:?}", plugin.entry);
    assert!(plugin.grants.contains(&Capability::OverrideModule("@app/builtin/demo".into())));
    let mut a = mount(&root, vec![plugin]);
    assert_eq!(a.error(), None);
    assert_eq!(a.text("label"), "the plugin's + the app's secret");
}

#[test]
fn a_plugin_you_installed_with_a_built_in_s_id_is_not_trusted() {
    let (root, builtin) = setup("refused");
    // Yours, laid out the same, in your plugins folder.
    let yours = root.join("data/plugins/demo");
    lay_out(&yours);
    let mine = |builtin_flag: bool, author: &str, at: &Path| item(&root, "on", builtin_flag, author, at);
    // As the engine lists it (not a built-in), or as a list that lies about
    // it: a built-in by Zeron, but from your folder; or from the app's
    // folder but not a built-in, or not Zeron's.
    for (case, listed) in [
        ("yours", mine(false, "user", &yours)),
        ("said built-in, your folder", mine(true, "zeron", &yours)),
        ("the app's folder, not a built-in", mine(false, "zeron", &builtin.join("demo"))),
        ("the app's folder, not Zeron's", mine(true, "user", &builtin.join("demo"))),
    ] {
        assert_eq!(listed.bundled(&builtin), None, "{case}");
        let plugin = listed.plugin_with(&builtin).expect("on: it loads");
        assert!(
            !plugin.grants.iter().any(|g| matches!(g, Capability::OverrideModule(_))),
            "{case}: {:?}",
            plugin.grants
        );
    }
    // Loaded from your folder, its `ui.override` is refused: the app keeps
    // its own module, and no plugin code ran as the app's.
    let yours_listed = mine(false, "user", &yours);
    let mut plugin = yours_listed.plugin_with(&builtin).unwrap();
    plugin.entry = yours.join("init.luau");
    let mut a = mount(&root, vec![plugin]);
    assert_eq!(a.text("label"), "the app's own");
    // Naming the module in "ui.replace" doesn't reach it either: only
    // screens are replaceable by a plugin you installed.
    let asked = Item {
        json: json!({
            "id": "demo", "status": "on", "entry": "init.luau", "snapshot": yours,
            "permissions": ["ui.replace"], "ui": { "replace": ["@app/builtin/demo"] },
        }),
    };
    assert!(!asked.plugin_with(&builtin).unwrap().grants.iter().any(|g| matches!(g, Capability::OverrideModule(_))));
}

#[test]
fn a_built_in_folder_reached_through_a_link_out_of_the_app_is_not_the_app_s() {
    let (root, builtin) = setup("linked");
    let elsewhere = root.join("elsewhere/demo");
    lay_out(&elsewhere);
    std::fs::remove_dir_all(builtin.join("demo")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&elsewhere, builtin.join("demo")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&elsewhere, builtin.join("demo")).unwrap();
    let listed = item(&root, "on", true, "zeron", &builtin.join("demo"));
    assert_eq!(listed.bundled(&builtin), None);
}

#[test]
fn a_bundled_built_in_turned_off_is_gone_with_the_next_load() {
    let (root, builtin) = setup("toggle");
    let on = item(&root, "on", true, "zeron", &builtin.join("demo"));
    let mut a = mount(&root, vec![on.plugin_with(&builtin).unwrap()]);
    assert_eq!(a.text("label"), "the plugin's + the app's secret");
    // Off (Settings → Plugins): not loaded, and the reload the bridge makes
    // finds the app's own module again.
    let off = item(&root, "off", true, "zeron", &builtin.join("demo"));
    assert!(off.plugin_with(&builtin).is_none());
    a.host.set_plugins(off.plugin_with(&builtin).into_iter().collect());
    a.host.reload().unwrap();
    a.frames(2);
    assert_eq!(a.error(), None);
    assert_eq!(a.text("label"), "the app's own");
    // And on again, without a restart.
    a.host.set_plugins(vec![on.plugin_with(&builtin).unwrap()]);
    a.host.reload().unwrap();
    a.frames(2);
    assert_eq!(a.text("label"), "the plugin's + the app's secret");
}

#[test]
fn a_bundled_built_in_may_name_app_modules_but_never_plugins_screens_or_everything() {
    for ok in ["@app/builtin/orchestrators", "@app/account/orchestration", "@app/screens/control/page", "@app/ds/orch_board"] {
        assert!(grants::bundled_replaceable(ok), "{ok}");
    }
    for never in [
        "*",
        "@app/*",
        "@app/",
        "@app/screens/plugins",
        "@app/screens/plugins/state",
        "@app/screens/shell/settings",
        "@app/screens/shell/settings/general",
        "@app/../outside",
        "@app/builtin//x",
        "@app/builtin/./x",
        "@ds/button",
        "builtin/orchestrators",
    ] {
        assert!(!grants::bundled_replaceable(never), "{never}");
    }
    let got = grants::bundled(&["@app/builtin/x".into(), "@app/screens/plugins".into(), "*".into()]);
    assert_eq!(got, vec![Capability::OverrideModule("@app/builtin/x".into())]);
}

/// What ships: Orchestrators' manifest names its app-side module, off by
/// default, and the app has the module it stands in for.
#[test]
fn orchestrators_ships_as_a_bundled_built_in() {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = app.join("plugins-builtin/orchestrators");
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("zeron-plugin.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "orchestrators");
    assert_eq!(manifest["switch"], json!({ "default": "off", "scope": "account" }));
    let modules: Vec<String> = serde_json::from_value(manifest["app_modules"].clone()).unwrap();
    assert!(!modules.is_empty());
    for module in &modules {
        assert!(grants::bundled_replaceable(module), "{module}");
        let rel = module.strip_prefix("@app/").unwrap();
        assert!(app.join("ui").join(format!("{rel}.luau")).is_file(), "the app's own {module}, for while it's off");
    }
    let listed = Item {
        json: json!({
            "id": "orchestrators", "status": "on", "entry": "init.luau", "builtin": true,
            "author": { "kind": "zeron" }, "dir": dir, "permissions": manifest["permissions"],
        }),
    };
    let bundled = listed.bundled(&app.join("plugins-builtin")).expect("bundled");
    assert_eq!(bundled.modules, modules);
}
