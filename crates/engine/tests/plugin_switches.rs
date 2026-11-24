//! Account-wide plugin switches across engines: a built-in whose manifest
//! makes its switch the account's (`"switch": { "default": "off", "scope":
//! "account" }`) is turned on in one engine's Settings → Plugins
//! (`SetPluginOff`), and a headless engine of the same account, which has
//! no window and so none of Zeron's own plugins' files, follows it through
//! the registry: `Plugins::is_on` reads it and `Plugins::watch` fires.
//!
//! The registry room is the in-memory mock server (what the edge's
//! RegistryRoom does over the wire); scratch data dirs, offline.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use zeron_engine::{EngineCore, HarnessId, HarnessRegistry};
use zeron_rpc::methods;

fn assemble(dir: &Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(dir).expect("create data dir");
    std::fs::write(dir.join("device-id"), device_id).expect("write device id");
    EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None)
        .expect("engine core assembles")
}

/// The app's folder of Zeron's own plugins, with one off by default whose
/// switch is the account's.
fn shipped(dir: &Path) -> std::path::PathBuf {
    let shipped = dir.join("app-plugins");
    let plugin = shipped.join("orchestrators");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("init.luau"), "return 1").unwrap();
    std::fs::write(
        plugin.join("zeron-plugin.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "orchestrators",
            "name": "Orchestrators",
            "version": "1.0.0",
            "entry": "init.luau",
            "switch": { "default": "off", "scope": "account" },
        }))
        .unwrap(),
    )
    .unwrap();
    shipped
}

async fn changed_to(rx: &mut tokio::sync::watch::Receiver<bool>, want: bool, what: &str) {
    tokio::time::timeout(Duration::from_secs(15), rx.wait_for(|on| *on == want))
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .expect("watch open");
}

fn status(list: &serde_json::Value, id: &str) -> Option<String> {
    list["plugins"]
        .as_array()?
        .iter()
        .find(|p| p["id"] == id)
        .and_then(|p| p["status"].as_str())
        .map(str::to_owned)
}

#[tokio::test]
async fn a_headless_engine_follows_the_account_s_plugin_switch() {
    let dir_desk = tempfile::tempdir().unwrap();
    let dir_box = tempfile::tempdir().unwrap();
    let desk = assemble(dir_desk.path(), "desk");
    let devbox = assemble(dir_box.path(), "devbox");
    let room = zeron_sync::registry::mock_server::MockRegistryServer::start().await;
    desk.workspace.connect_registry_url(&room.url());
    devbox.workspace.connect_registry_url(&room.url());

    // The desk's window says where Zeron's own are; the devbox has none.
    let desk_rpc = zeron_rpc::memory_client(desk.rpc_service());
    let shipped = shipped(dir_desk.path());
    let list = desk_rpc
        .call(
            methods::SET_BUILTIN_PLUGINS,
            serde_json::json!({ "dir": shipped }),
        )
        .await
        .expect("SetBuiltinPlugins");
    assert_eq!(
        status(&list, "orchestrators").as_deref(),
        Some("off"),
        "off on a fresh install: {list}"
    );
    assert!(!desk.plugins.is_on("orchestrators"));
    assert!(!devbox.plugins.is_on("orchestrators"));
    let mut on_box = devbox.plugins.watch("orchestrators");
    let mut on_desk = desk.plugins.watch("orchestrators");
    assert!(!*on_box.borrow());

    // Turned on in the desk's Settings → Plugins.
    let item = desk_rpc
        .call(
            methods::SET_PLUGIN_OFF,
            serde_json::json!({ "id": "orchestrators", "off": false }),
        )
        .await
        .expect("SetPluginOff");
    assert_eq!(item["status"], "on", "{item}");
    assert_eq!(item["scope"], "account", "{item}");
    changed_to(&mut on_desk, true, "the desk's own watch").await;
    changed_to(
        &mut on_box,
        true,
        "the switch to land on the headless engine",
    )
    .await;
    assert!(devbox.plugins.is_on("orchestrators"));

    // And off again, from the desk: the devbox follows.
    desk_rpc
        .call(
            methods::SET_PLUGIN_OFF,
            serde_json::json!({ "id": "orchestrators", "off": true }),
        )
        .await
        .expect("SetPluginOff");
    changed_to(&mut on_box, false, "off to land on the headless engine").await;
    assert!(!devbox.plugins.is_on("orchestrators"));
    // The desk's list says what's in effect.
    let list = desk_rpc
        .call(methods::LIST_PLUGINS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        status(&list, "orchestrators").as_deref(),
        Some("off"),
        "{list}"
    );
}
