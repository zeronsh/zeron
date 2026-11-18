//! Plugins from the store, end to end on one device: a real engine plugin
//! service (scratch data dir) talking HTTP to a stand-in for the edge's
//! store (`support/fake_store.rs`). Install by id, an update signed by
//! another of the owner's keys, the owner's own pending version, a blocked
//! version, a version made for another Zeron, a new owner, a package that
//! isn't what the store's record says, a hostile store, and the store
//! offline. Other sources aren't touched: their tests are the engine's own.

#[path = "support/fake_store.rs"]
mod fake_store;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use fake_store::FakeStore;
use zeron_engine::plugins::store::client::StoreClient;
use zeron_engine::plugins::store::deploy_key::DeployKey;
use zeron_engine::plugins::{Plugins, install, package, signing::SigningKey};
use zeron_rpc::{RpcReply, StaticToken, methods};

/// A package of plugin `id` at `version`, signed by `key`.
fn package_of(key: &SigningKey, id: &str, version: &str, permissions: &[&str], body: &str, extra: Value) -> Vec<u8> {
    let files: BTreeMap<String, Vec<u8>> = [("init.luau".to_string(), body.as_bytes().to_vec())].into();
    let mut manifest = json!({ "id": id, "name": "Focus", "version": version, "entry": "init.luau", "permissions": permissions });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        manifest[k] = v;
    }
    let manifest = key.sign_manifest(&manifest, &package::hashes(&files));
    package::pack(install::FORMAT, &manifest, &files).unwrap()
}

struct Device {
    _dir: tempfile::TempDir,
    plugins: Plugins,
}

/// A device signed in as `username` (`None`: signed out), with the store.
fn device(store: &FakeStore, username: Option<&str>) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), "dev-1");
    let session: Option<Arc<dyn zeron_rpc::TokenSource>> = username.map(|u| Arc::new(StaticToken(format!("session:{u}"))) as _);
    plugins.set_store(StoreClient::new(&store.url), session);
    Device { _dir: dir, plugins }
}

impl Device {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        match self.plugins.dispatch(method, params).await {
            Ok(RpcReply::Value(value)) => Ok(value),
            Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn item(&self, id: &str) -> Value {
        let list = self.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
        list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap_or(Value::Null)
    }

    /// Allowed as Zeron's consent window does: a review minted for it as shown.
    async fn allow(&self, id: &str) -> Value {
        let hash = self.item(id).await["hash"].clone();
        let review = self.call(methods::REVIEW_PLUGIN, json!({ "id": id, "hash": hash })).await.unwrap()["review"].clone();
        self.call(methods::DECIDE_PLUGIN, json!({ "id": id, "allow": true, "hash": hash, "review": review })).await.unwrap()
    }

    async fn check(&self) -> Value {
        self.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap()
    }
}

/// The reason word a failure carries.
fn word(error: &str) -> &str {
    let error = error.trim_start_matches("failed: ").trim_start_matches("Failed: ");
    install::reason_of(error).unwrap_or(error)
}

#[tokio::test]
async fn install_from_the_store_and_update_by_the_owners_other_key() {
    let store = FakeStore::start().await;
    let deploy = DeployKey::from_seed([5; 32]).unwrap();
    let (laptop, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(deploy.signing_key(), "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let d = device(&store, Some("bob"));

    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(installed["update"], false);
    assert_eq!(installed["waiting"], true, "someone else's plugin waits for the person");
    let item = &installed["plugin"];
    assert_eq!(item["author"], json!({ "kind": "verified", "username": "ada" }), "who made it: the store's record");
    assert_eq!(item["source"], json!({ "kind": "store", "id": "focus", "publisher": "ada" }));
    assert_eq!(d.allow("focus").await["status"], "on");
    // Counted once, in the background.
    for _ in 0..50 {
        if store.installs("focus") == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(store.installs("focus"), 1);

    // 1.1.0, signed on the owner's laptop: another key, the same owner.
    store.add("ada", &package_of(&laptop, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "approved", "- Faster.");
    let checked = d.check().await;
    assert_eq!(checked["errors"], json!([]), "{checked}");
    let update = &checked["updates"][0];
    assert_eq!((update["id"].as_str(), update["version"].as_str(), update["notes"].as_str()), (Some("focus"), Some("1.1.0"), Some("- Faster.")));
    assert!(update.get("new_owner").is_none() && update.get("in_review").is_none());
    // One request for every store plugin, saying this Zeron's version.
    let ask = store.state.lock().unwrap().asks.last().cloned().unwrap();
    assert_eq!(ask["plugins"], json!([{ "id": "focus", "version": "1.0.0", "hash": item["hash"] }]));
    assert_eq!(ask["zeron"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d.item("focus").await["update"]["version"], "1.1.0");

    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!((updated["update"].as_bool(), updated["from"].as_str()), (Some(true), Some("1.0.0")));
    assert_eq!(updated["plugin"]["status"], "on", "the same owner's update keeps what you allowed");
    assert_eq!(updated["plugin"]["version"], "1.1.0");
    // Up to date now: nothing offered, nothing kept.
    assert_eq!(d.check().await["updates"], json!([]));
    assert!(d.item("focus").await.get("update").is_none());
    assert_eq!(store.installs("focus"), 1, "an update isn't another install");
}

#[tokio::test]
async fn the_owner_s_own_devices_are_offered_their_pending_version() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "approved", "");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &[], "return 2", json!({})), "pending", "- Trying.");
    // Someone else's device: nothing in review is offered.
    let theirs = device(&store, Some("bob"));
    theirs.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(theirs.check().await["updates"], json!([]));
    assert!(theirs.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.1.0" })).await.is_err());
    // The owner's other device: offered, marked as in review.
    let mine = device(&store, Some("ada"));
    mine.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    let checked = mine.check().await;
    assert_eq!(checked["updates"][0]["version"], "1.1.0");
    assert_eq!(checked["updates"][0]["in_review"], true);
    assert_eq!(mine.item("focus").await["update"]["in_review"], true);
    let updated = mine.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["plugin"]["version"], "1.1.0");
}

#[tokio::test]
async fn a_blocked_version_is_turned_off_and_can_be_turned_back_on() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");

    store.block("focus", "1.0.0", "It sent your chats somewhere.");
    let checked = d.check().await;
    assert_eq!(checked["errors"], json!([]));
    let item = d.item("focus").await;
    assert_eq!(item["status"], "off");
    assert_eq!(item["blocked"]["reason"], "It sent your chats somewhere.");
    assert_eq!(item["blocked"]["version"], "1.0.0");
    let notices = d.call(methods::LIST_PLUGINS, json!({})).await.unwrap()["key_notices"].clone();
    assert!(notices.as_array().unwrap().iter().any(|n| n["kind"] == "plugin_blocked"), "{notices}");

    // It's theirs: turned back on, and the next check leaves it on.
    let on = d.call(methods::SET_PLUGIN_OFF, json!({ "id": "focus", "off": false })).await.unwrap();
    assert_eq!(on["status"], "on");
    assert_eq!(on["blocked"]["overridden"], true, "the reason is still said");
    d.check().await;
    assert_eq!(d.item("focus").await["status"], "on");
}

#[tokio::test]
async fn a_version_for_another_zeron_is_never_installed() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "approved", "");
    let future = package_of(&key, "focus", "2.0.0", &[], "return 2", json!({ "engines": { "zeron": ">=99.0.0" } }));
    store.add("ada", &future, "approved", "");
    let d = device(&store, Some("bob"));
    // Asked for by version: refused with its reason.
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "2.0.0" })).await.unwrap_err();
    assert_eq!(word(&refused), "incompatible", "{refused}");
    assert!(d.item("focus").await.is_null());
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.0.0" })).await.unwrap();
    // An honest store doesn't offer it; a store that does is refused.
    assert_eq!(d.check().await["updates"], json!([]));
    let package = install::parse(future.clone()).unwrap();
    store.state.lock().unwrap().offer_anyway.push(json!({
        "id": "focus", "version": "2.0.0", "hash": package.hash, "sha256": sha256(&future), "bytes": future.len(), "key": package.key, "publisher": "ada", "publisher_id": fake_store::publisher_id("ada"),
    }));
    let checked = d.check().await;
    let refused = checked["updates"][0]["refused"].as_str().unwrap_or_default();
    assert_eq!(word(refused), "incompatible", "{checked}");
    let refused = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "incompatible");
    assert_eq!(d.item("focus").await["version"], "1.0.0");
    // The same holds for a plugin from a file.
    let dir = tempfile::tempdir().unwrap();
    let other = package_of(&key, "later", "1.0.0", &[], "return 1", json!({ "engines": { "zeron": "^99.0.0" } }));
    std::fs::write(dir.path().join("later.zeron-plugin"), other).unwrap();
    let path = dir.path().join("later.zeron-plugin").display().to_string();
    assert_eq!(word(&d.call(methods::INSTALL_PLUGIN, json!({ "path": path })).await.unwrap_err()), "incompatible");
}

#[tokio::test]
async fn a_new_owner_asks_again() {
    let store = FakeStore::start().await;
    let (ada, _) = SigningKey::generate().unwrap();
    let (cy, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&ada, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");

    store.transfer("focus", "cy");
    store.add("cy", &package_of(&cy, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "approved", "");
    let checked = d.check().await;
    assert_eq!(checked["updates"][0]["new_owner"], true, "{checked}");
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["plugin"]["author"]["username"], "cy");
    assert_eq!(updated["plugin"]["status"], "pending", "a new owner waits for the person again");
    assert_eq!(d.allow("focus").await["status"], "on");
}

#[tokio::test]
async fn a_package_that_isn_t_the_store_s_record_is_refused() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let real = package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({}));
    store.add("ada", &real, "approved", "");
    let d = device(&store, None);

    // Another build than the record's (validly signed, even).
    let other = package_of(&key, "focus", "1.0.0", &["ui.add", "network"], "return 666", json!({}));
    store.state.lock().unwrap().serve_instead.insert(("focus".into(), "1.0.0".into()), other);
    assert_eq!(word(&d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err()), "store_mismatch");
    // Changed after it was signed.
    let mut value: Value = serde_json::from_slice(&real).unwrap();
    value["files"]["init.luau"] = json!("cmV0dXJuIDI=");
    store.state.lock().unwrap().serve_instead.insert(("focus".into(), "1.0.0".into()), serde_json::to_vec(&value).unwrap());
    assert_eq!(word(&d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err()), "store_mismatch");
    // Nothing was written.
    assert!(d.item("focus").await.is_null());
    // Signed out, a public plugin still installs.
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.item("focus").await["version"], "1.0.0");

    // A hostile store: an older version, a plugin not installed here, a
    // record without its publisher.
    let older = package_of(&key, "focus", "0.9.0", &[], "return 0", json!({}));
    let older_package = install::parse(older.clone()).unwrap();
    store.state.lock().unwrap().serve_instead.insert(("focus".into(), "0.9.0".into()), older.clone());
    let offer = |version: &str, raw: &[u8], hash: &str, publisher: Option<&str>| {
        json!({ "id": "focus", "version": version, "hash": hash, "sha256": sha256(raw), "bytes": raw.len(), "publisher": publisher, "publisher_id": fake_store::publisher_id("ada") })
    };
    store.state.lock().unwrap().offer_anyway.push(offer("0.9.0", &older, &older_package.hash, Some("ada")));
    let checked = d.check().await;
    assert_eq!(word(checked["updates"][0]["refused"].as_str().unwrap()), "not_newer", "{checked}");
    assert!(d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.is_err());
    store.state.lock().unwrap().offer_anyway.push(json!({ "id": "someone-else", "version": "9.0.0", "hash": "00", "publisher": "eve" }));
    let checked = d.check().await;
    assert_eq!((checked["updates"].clone(), checked["errors"].clone()), (json!([]), json!([])), "an offer for a plugin not asked about is ignored");
    let later = package_of(&key, "focus", "1.1.0", &[], "return 3", json!({}));
    let later_package = install::parse(later.clone()).unwrap();
    store.state.lock().unwrap().serve_instead.insert(("focus".into(), "1.1.0".into()), later.clone());
    store.state.lock().unwrap().offer_anyway.push(offer("1.1.0", &later, &later_package.hash, None));
    let checked = d.check().await;
    assert_eq!(word(checked["errors"][0]["error"].as_str().unwrap()), "store_mismatch", "{checked}");
    assert_eq!(d.item("focus").await["version"], "1.0.0");
}

#[tokio::test]
async fn offline_the_installed_version_keeps_running() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    d.allow("focus").await;
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let checked = d.check().await;
    assert_eq!(checked["updates"], json!([]));
    assert_eq!(word(checked["errors"][0]["error"].as_str().unwrap()), "store_unreachable", "{checked}");
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["author"]["username"].as_str()), (Some("on"), Some("ada")), "who made it needs no network");
    assert_eq!(word(&d.call(methods::INSTALL_PLUGIN, json!({ "store": "other" })).await.unwrap_err()), "store_unreachable");
    // Categories still answer, from the list this Zeron knows.
    let categories = d.call(methods::STORE_CATEGORIES, json!({})).await.unwrap();
    assert!(categories["categories"].as_array().unwrap().len() >= 8);
}

#[tokio::test]
async fn a_store_plugin_changed_on_disk_is_told_by_its_signature_alone() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let d = device(&store, Some("bob"));
    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    d.allow("focus").await;
    let dir = installed["plugin"]["dir"].as_str().unwrap().to_owned();
    std::fs::write(std::path::Path::new(&dir).join("init.luau"), "return 'evil'").unwrap();
    let item = d.item("focus").await;
    assert_ne!(item["author"]["username"], "ada", "no longer what the store vouched for: {item}");
    assert_ne!(item["status"], "on");
    // Reinstalling fetches the version installed from the store again.
    let again = d.call(methods::INSTALL_PLUGIN, json!({ "reinstall": "focus" })).await.unwrap();
    assert_eq!(again["plugin"]["author"]["username"], "ada");
    assert_eq!(again["plugin"]["status"], "pending", "a reinstall asks again");
}

fn sha256(bytes: &[u8]) -> String {
    zeron_engine::plugins::signing::sha256_hex(bytes)
}

#[tokio::test]
async fn the_owner_installs_a_plugin_nobody_else_sees_yet() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "pending", "");
    // Not listed to anyone yet: someone else's device can't have it.
    let theirs = device(&store, Some("bob"));
    assert_eq!(word(&theirs.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err()), "not_in_store");
    // Its owner's other device can, from their plugins in the store.
    let mine = device(&store, Some("ada"));
    let installed = mine.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!((installed["plugin"]["version"].as_str(), installed["plugin"]["author"]["username"].as_str()), (Some("1.0.0"), Some("ada")));
    assert_eq!(installed["waiting"], true, "installed from the store, it's asked about like anyone's");
}

#[tokio::test]
async fn an_offer_the_device_can_t_read_hides_nothing_else() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    store.add("ada", &package_of(&key, "board", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let next = package_of(&key, "board", "1.1.0", &["ui.add"], "return 2", json!({}));
    store.add("ada", &next, "approved", "");
    let d = device(&store, Some("bob"));
    for id in ["focus", "board"] {
        d.call(methods::INSTALL_PLUGIN, json!({ "store": id, "version": "1.0.0" })).await.unwrap();
        d.allow(id).await;
    }
    // board's offer with `notes: null` (no CHANGELOG section), an offer
    // with no hash, and focus's installed version blocked.
    store.block("focus", "1.0.0", "Taken down.");
    let package = install::parse(next.clone()).unwrap();
    {
        let mut state = store.state.lock().unwrap();
        // (Offered only as the hostile shape below.)
        state.plugins.get_mut("board").unwrap().versions.retain(|v| v.version != "1.1.0");
        state.offer_anyway.push(json!({ "id": "board", "version": "1.1.0", "hash": package.hash, "sha256": sha256(&next), "bytes": next.len(), "key": package.key, "publisher": "ada", "publisher_id": fake_store::publisher_id("ada"), "notes": null, "added": [], "removed": [] }));
        state.offer_anyway.push(json!({ "id": "focus", "version": "2.0.0", "publisher": "ada" }));
        state.serve_instead.insert(("board".into(), "1.1.0".into()), next.clone());
    }
    let checked = d.check().await;
    assert_eq!(d.item("focus").await["status"], "off", "the block still lands: {checked}");
    let board = checked["updates"].as_array().unwrap().iter().find(|u| u["id"] == "board").cloned().unwrap();
    assert_eq!(board["version"], "1.1.0");
    assert!(board.get("notes").is_none(), "no notes: {board}");
    assert!(checked["errors"].as_array().unwrap().iter().any(|e| e["id"] == "focus" && word(e["error"].as_str().unwrap()) == "store_error"), "{checked}");
}

#[tokio::test]
async fn reports_compatibility_for_another_zeron_and_deploy_keys_for_owned_plugins_only() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({ "engines": { "zeron": ">=0.5.0" } })), "approved", "");
    let d = device(&store, Some("bob"));
    let here = d.call(methods::STORE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    let there = d.call(methods::STORE_PLUGIN, json!({ "id": "focus", "for_zeron": "0.5.1" })).await.unwrap();
    assert_eq!(there["compatible"], true, "{there}");
    assert_eq!(here["compatible"], version_at_least("0.5.0"), "{here}");
    assert!(d.call(methods::STORE_PLUGIN, json!({ "id": "focus", "for_zeron": "soon" })).await.is_err());

    let reported = d.call(methods::STORE_REPORT, json!({ "id": "focus", "reason": "malware", "text": "It sends\u{202e} my chats away." })).await.unwrap();
    assert_eq!(reported["ok"], true);
    let (id, body) = store.state.lock().unwrap().reports[0].clone();
    assert_eq!((id.as_str(), body["reason"].as_str()), ("focus", Some("malware")));
    assert!(!body["text"].as_str().unwrap().contains('\u{202e}'));
    assert!(d.call(methods::STORE_REPORT, json!({ "id": "focus", "reason": "boring" })).await.is_err());

    // bob owns no focus in the store: no deploy key for it.
    let refused = d.call(methods::CREATE_DEPLOY_KEY, json!({ "id": "focus", "name": "CI" })).await.unwrap();
    assert_eq!(refused["error"], "not_published", "{refused}");
}

/// Whether this Zeron is at least `v`.
fn version_at_least(v: &str) -> bool {
    use zeron_engine::plugins::version::{Version, zeron};
    zeron() >= Version::parse(v).unwrap()
}

#[tokio::test]
async fn a_store_file_is_fetched_once_and_only_from_inside_its_plugin() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "approved", "");
    let d = device(&store, None);
    let shown = d.call(methods::STORE_FILE, json!({ "id": "focus", "version": "1.0.0", "path": "init.luau" })).await.unwrap();
    assert_eq!(std::fs::read_to_string(shown["file"].as_str().unwrap()).unwrap(), "return 1");
    assert_eq!(shown["bytes"], 8);
    let asked = store.state.lock().unwrap().requests.len();
    let again = d.call(methods::STORE_FILE, json!({ "id": "focus", "version": "1.0.0", "path": "init.luau" })).await.unwrap();
    assert_eq!(again["file"], shown["file"]);
    assert_eq!(store.state.lock().unwrap().requests.len(), asked, "from the cache");
    for (version, path) in [("1.0.0", "../../escape"), ("1.0.0", "zeron-plugin.json"), ("1.0.0", "/etc/passwd"), ("../1.0.0", "init.luau"), ("1.0.0", ".git/config")] {
        assert!(d.call(methods::STORE_FILE, json!({ "id": "focus", "version": version, "path": path })).await.is_err(), "{version} {path}");
    }
    assert_eq!(store.state.lock().unwrap().requests.len(), asked, "nothing asked for outside the plugin");
}

/// Consent is bound to the owner's stable id, never the username: an
/// operator moving "ada" to another account (which then owns the plugin)
/// is a new owner, and asks again, though every name reads the same.
#[tokio::test]
async fn the_same_username_on_another_account_asks_again() {
    let store = FakeStore::start().await;
    let (ada, _) = SigningKey::generate().unwrap();
    let (other, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&ada, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "approved", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");

    store.move_username("focus", "someone-else");
    store.add("ada", &package_of(&other, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "approved", "");
    let checked = d.check().await;
    assert_eq!(checked["updates"][0]["new_owner"], true, "{checked}");
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["plugin"]["author"]["username"], "ada", "the name people read is the same");
    assert_eq!(updated["plugin"]["status"], "pending", "but it's another account: asked again");
}
