//! Where the Rust tests find a Luau fixture: ui/'s own (`ui/<name>`), or a
//! built-in plugin's (`plugins-builtin/<id>/tests/<name>`), mounted in a
//! copy of ui/ made for that plugin the way scripts/ci/test-zgx.sh stages
//! its Luau tests (`plugin_tests`): ui/ without its own tests, the plugin's
//! fixtures and worlds beside app.luau, an alias of its id naming its app/
//! folder (`@orchestrators/…`), and a zgx.json loading its overrides.luau
//! with the grants its manifest's `app_modules` give, so the fixture reaches
//! the plugin's app-side modules as the app does. A test names the fixture
//! alone, so it finds it on either side of a move into a plugin.
//!
//! A plugin's copy is made once a test process, in the target folder
//! (`plugin-stages/<id>-<pid>`), its files hard links where they can be;
//! ones a day old are cleared as a new one is made.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use zgx::reactive::Runtime;
use zgx_script::ScriptHost;

fn app() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The host of fixture `name`, wherever it is (see the module's comment).
pub(crate) fn host(runtime: &Runtime, name: &str) -> ScriptHost {
    let path = path(name);
    configured(ScriptHost::new(runtime, &path), &path)
}

/// `host`, of the fixture at `path`, given what its folder's zgx.json says
/// (a plugin's stage has one; ui/ none), as `zgx test` and `zgx run` do.
pub(crate) fn configured(host: ScriptHost, path: &Path) -> ScriptHost {
    let dir = path.parent().expect("a fixture's folder");
    let config = zgx_script::app_config::read(dir).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    config.apply(host)
}

/// Fixture `name`'s path: ui/'s, or in its plugin's stage.
pub(crate) fn path(name: &str) -> PathBuf {
    let ui = app().join("ui").join(name);
    if ui.is_file() {
        return ui;
    }
    let builtin = app().join("plugins-builtin");
    let mut owners = std::fs::read_dir(&builtin)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|plugin| plugin.join("tests").join(name).is_file());
    let plugin = owners.next().unwrap_or_else(|| panic!("no fixture {name} in ui/ nor in plugins-builtin/*/tests"));
    stage(&plugin).join(name)
}

static STAGES: Mutex<Option<HashMap<PathBuf, PathBuf>>> = Mutex::new(None);

fn stage(plugin: &Path) -> PathBuf {
    // Its real path, as the script's (zgx resolves a replacement's folder
    // through links).
    let plugin = plugin.canonicalize().expect("the plugin's folder");
    let mut stages = STAGES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let stages = stages.get_or_insert_with(HashMap::new);
    if let Some(stage) = stages.get(&plugin) {
        return stage.clone();
    }
    let id = plugin.file_name().and_then(|n| n.to_str()).expect("the plugin's id").to_string();
    let exe = std::env::current_exe().expect("the test binary");
    // target/<profile>/deps/<binary> -> target/<profile>/plugin-stages
    let root = exe.parent().and_then(Path::parent).expect("the target folder").join("plugin-stages");
    std::fs::create_dir_all(&root).expect("plugin-stages");
    clear_old(&root);
    let stage = root.join(format!("{id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    copy_tree(&app().join("ui"), &stage, true);
    std::fs::create_dir_all(stage.join("tests")).expect("the stage's tests");
    if plugin.join("app").is_dir() {
        let luaurc = stage.join(".luaurc");
        let mut config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&luaurc).expect("ui/.luaurc")).expect("ui/.luaurc is JSON");
        config["aliases"][id.as_str()] = serde_json::Value::from(plugin.join("app").to_string_lossy().into_owned());
        // A link to ui/'s own (`copy_tree`): written through, it would be
        // ui/'s that changed. The stage's is a file of its own.
        std::fs::remove_file(&luaurc).expect("the stage's .luaurc");
        std::fs::write(&luaurc, serde_json::to_vec_pretty(&config).unwrap()).expect("the stage's .luaurc");
    }
    for entry in std::fs::read_dir(plugin.join("tests")).into_iter().flatten().flatten() {
        let file = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".luau") || name.ends_with(".test.luau") {
            continue;
        }
        let to = stage.join(&name);
        assert!(!to.exists(), "{name} is the app's too: name {id}'s fixture its own ({})", file.display());
        link_or_copy(&file, &to);
    }
    if plugin.join("overrides.luau").is_file() {
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(plugin.join("zeron-plugin.json")).expect("the plugin's manifest"),
        )
        .expect("the manifest is JSON");
        let modules: Vec<String> = manifest["app_modules"]
            .as_array()
            .map(|m| m.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let config = serde_json::json!({
            "expose": { "hooks": modules.iter().map(|m| (m.clone(), serde_json::json!("override"))).collect::<serde_json::Map<_, _>>() },
            "plugins": { manifest["id"].as_str().expect("the manifest's id"): {
                "path": plugin.join("overrides.luau").to_string_lossy(),
                "allow": modules.iter().map(|m| format!("override-module:{m}")).collect::<Vec<_>>(),
            }},
        });
        std::fs::write(stage.join("zgx.json"), serde_json::to_vec_pretty(&config).unwrap()).expect("the stage's zgx.json");
    }
    stages.insert(plugin, stage.clone());
    stage
}

/// `from` into `to`, leaving out its top-level tests/ when `top`.
fn copy_tree(from: &Path, to: &Path, top: bool) {
    std::fs::create_dir_all(to).expect("a stage folder");
    for entry in std::fs::read_dir(from).expect("ui/").flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            if top && entry.file_name() == "tests" {
                continue;
            }
            copy_tree(&path, &target, false);
        } else {
            link_or_copy(&path, &target);
        }
    }
}

fn link_or_copy(from: &Path, to: &Path) {
    if std::fs::hard_link(from, to).is_err() {
        std::fs::copy(from, to).unwrap_or_else(|e| panic!("{} -> {}: {e}", from.display(), to.display()));
    }
}

/// Stages a day old, from runs that are long over.
fn clear_old(root: &Path) {
    let day = Duration::from_secs(24 * 60 * 60);
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age > day);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The plugin's own fixture, mounted through its stage: the gated hook
/// draws the plugin's module once `plugins.on` says it's on.
#[test]
fn a_plugin_s_fixture_mounts_with_its_overrides_loaded() {
    use zgx_script::testing::TestApp;
    let path = path("orchestrators_proof_fixture.luau");
    assert!(!path.starts_with(app().join("ui")), "{}: the plugin's, staged", path.display());
    // The stage's .luaurc names the plugin's app/; ui/'s, linked into the
    // stage with the rest, doesn't.
    let ours = std::fs::read_to_string(path.with_file_name(".luaurc")).expect("the stage's .luaurc");
    assert!(ours.contains("\"orchestrators\""), "{ours}");
    let ui = std::fs::read_to_string(app().join("ui/.luaurc")).expect("ui/.luaurc");
    assert!(!ui.contains("\"orchestrators\""), "ui/.luaurc written through the stage's link: {ui}");
    let mut app = TestApp::new(|runtime| host(runtime, "orchestrators_proof_fixture.luau"));
    app.frames(2);
    assert_eq!(app.error(), None, "the fixture failed");
    app.host.set_state("orch_proof_on", zgx_script::Value::Bool(true)).expect("the state");
    app.frames(2);
    assert_eq!(app.text("#orch-proof"), "Orchestrators");
}
