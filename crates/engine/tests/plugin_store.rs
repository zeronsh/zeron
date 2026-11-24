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
use zeron_engine::plugins::store::deploy_token::{DeployToken, Scope};
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

/// [`package_of`] unsigned: as a deploy token publishes it.
fn unsigned_package_of(id: &str, version: &str, permissions: &[&str], body: &str) -> Vec<u8> {
    let files: BTreeMap<String, Vec<u8>> = [("init.luau".to_string(), body.as_bytes().to_vec())].into();
    let manifest = json!({ "id": id, "name": "Focus", "version": version, "entry": "init.luau", "permissions": permissions });
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

    /// `method` as `caller` sends it (an agent's MCP server, another device).
    async fn call_as(&self, caller: zeron_rpc::Caller, method: &str, params: Value) -> Result<Value, String> {
        zeron_rpc::with_caller(caller, self.call(method, params)).await
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
    let (desk, _) = SigningKey::generate().unwrap();
    let (laptop, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&desk, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));

    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(installed["update"], false);
    assert_eq!(installed["waiting"], true, "someone else's plugin waits for the person");
    let item = &installed["plugin"];
    assert_eq!(item["author"], json!({ "kind": "verified", "username": "ada" }), "who made it: the store's record");
    assert_eq!(item["source"], json!({ "kind": "store", "id": "focus", "publisher": "ada", "verified": true, "verified_at": 1 }));
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
    store.add("ada", &package_of(&laptop, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "- Faster.");
    let checked = d.check().await;
    assert_eq!(checked["errors"], json!([]), "{checked}");
    let update = &checked["updates"][0];
    assert_eq!((update["id"].as_str(), update["version"].as_str(), update["notes"].as_str()), (Some("focus"), Some("1.1.0"), Some("- Faster.")));
    assert!(update.get("new_owner").is_none() && update.get("in_review").is_none());
    assert_eq!(update["new_key"], true, "signed on the owner's laptop: the update says its key changed");
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
async fn an_unverified_update_is_offered_at_once_marked_and_always_asks() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    // 1.1.0 verified, 1.2.0 published since and not: by default the newest
    // is staged, marked, the verified one beside it.
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    store.add("ada", &package_of(&key, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "listed", "- Fresh.");
    let checked = d.check().await;
    let update = &checked["updates"][0];
    assert_eq!((update["version"].as_str(), update["verified"].as_bool(), update["installed_verified"].as_bool()), (Some("1.2.0"), Some(false), Some(true)), "{checked}");
    assert_eq!(update["verified_alternative"], json!({ "version": "1.1.0", "added": [], "removed": [], "verified_at": 1 }));
    assert!(update.get("new_key").is_none(), "signed by the key installed");
    // As one of many ("update all"), it's never taken.
    let refused = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus", "verified_only": true })).await.unwrap_err();
    assert_eq!(word(&refused), "not_verified", "{refused}");
    assert_eq!(d.item("focus").await["version"], "1.0.0");
    // On its own: taken, and it asks, though it asks for nothing new.
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!((updated["plugin"]["version"].as_str(), updated["plugin"]["status"].as_str()), (Some("1.2.0"), Some("pending")), "{updated}");
    let item = d.item("focus").await;
    assert_eq!((item["source"]["verified"].as_bool(), item["nothing_new"].as_bool()), (Some(false), Some(true)), "the sheet says it's not verified and asks nothing new: {item}");
    // And says of the version it replaced: "The version you have, 1.0.0,
    // was verified."
    assert_eq!(item["source"]["replaced"], json!({ "version": "1.0.0", "verified": true }), "{item}");
    assert!(item.get("digest_diff").is_none());
    // A first install of an unverified version isn't "nothing new".
    let other = device(&store, Some("eve"));
    let fresh = other.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.2.0" })).await.unwrap();
    assert!(fresh["plugin"].get("nothing_new").is_none(), "{fresh}");
}

#[tokio::test]
async fn the_device_stages_the_verified_update_when_told_to() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    store.add("ada", &package_of(&key, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "listed", "");
    // The default is the newest; "verified" stages 1.1.0.
    let set = d.call(methods::SET_PLUGIN_UPDATES, json!({ "id": "focus", "prefer": "verified" })).await.unwrap();
    assert_eq!((set["prefer"].as_str(), set["device"].as_str()), (Some("verified"), Some("newest")));
    let list = d.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    assert_eq!(list["store_updates"], json!({ "device": "newest", "plugins": { "focus": "verified" } }), "the list says it");
    let checked = d.check().await;
    assert_eq!((checked["updates"][0]["version"].as_str(), checked["updates"][0]["verified"].as_bool()), (Some("1.1.0"), Some(true)), "{checked}");
    // A verified update keeps today's rule: nothing new asked, it stays on.
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus", "verified_only": true })).await.unwrap();
    assert_eq!((updated["plugin"]["version"].as_str(), updated["plugin"]["status"].as_str()), (Some("1.1.0"), Some("on")));
    // Only an unverified one newer: held, said, not staged.
    let checked = d.check().await;
    assert_eq!((checked["updates"][0]["version"].as_str(), checked["updates"][0]["held"].as_bool()), (Some("1.2.0"), Some(true)), "{checked}");
    assert!(d.item("focus").await.get("update").is_none());
    // Back to the device's: the newest is staged.
    d.call(methods::SET_PLUGIN_UPDATES, json!({ "id": "focus", "prefer": null })).await.unwrap();
    assert_eq!(d.check().await["updates"][0]["version"], "1.2.0");
    assert!(d.call(methods::SET_PLUGIN_UPDATES, json!({ "prefer": "oldest" })).await.is_err());
}

#[tokio::test]
async fn a_blocked_version_is_turned_off_and_can_be_turned_back_on() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
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

#[tokio::test(flavor = "multi_thread")]
async fn a_block_repeated_by_a_hostile_store_is_applied_once() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    // 8 MB, so each rescan (which hashes every file) costs something.
    let body = format!("return 1 --{}", "x".repeat(8 * 1024 * 1024));
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], &body, json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let mut blocks = vec![json!({ "id": "focus", "version": "1.0.0", "reason": "It sent your chats somewhere." }); 2000];
    blocks.push(json!({ "id": "focus", "version": "1.0.0", "reason": "Another reason." }));
    store.state.lock().unwrap().blocked_anyway = blocks;
    let started = std::time::Instant::now();
    d.check().await;
    let took = started.elapsed();
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("It sent your chats somewhere.")), "{item}");
    // (A rescan per copy took over ten seconds here.)
    assert!(took < Duration::from_secs(3), "2001 copies of one block took {took:?}");
}

#[tokio::test]
async fn a_version_for_another_zeron_is_never_installed() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "verified", "");
    let future = package_of(&key, "focus", "2.0.0", &[], "return 2", json!({ "engines": { "zeron": ">=99.0.0" } }));
    store.add("ada", &future, "verified", "");
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
        "verified": true,
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
    // One that names no `engines` installs exactly as before.
    let plain = package_of(&key, "plain", "1", &[], "return 1", json!({}));
    std::fs::write(dir.path().join("plain.zeron-plugin"), plain).unwrap();
    let path = dir.path().join("plain.zeron-plugin").display().to_string();
    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "path": path })).await.unwrap();
    assert_eq!((installed["plugin"]["version"].as_str(), installed["update"].as_bool()), (Some("1"), Some(false)));
}

#[tokio::test]
async fn a_new_owner_asks_again() {
    let store = FakeStore::start().await;
    let (ada, _) = SigningKey::generate().unwrap();
    let (cy, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&ada, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");

    store.transfer("focus", "cy");
    store.add("cy", &package_of(&cy, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    let checked = d.check().await;
    assert_eq!(checked["updates"][0]["new_owner"], true, "{checked}");
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["plugin"]["author"]["username"], "cy");
    assert_eq!(updated["plugin"]["status"], "pending", "a new owner waits for the person again");
    assert_eq!(d.allow("focus").await["status"], "on");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_package_is_read_no_further_than_its_record_says() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    // 60 MB where the record says a few hundred bytes: refused at the size,
    // not read to the end and hashed.
    store.state.lock().unwrap().serve_instead.insert(("focus".into(), "1.0.0".into()), vec![b' '; 60 * 1024 * 1024]);
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch");
    assert!(refused.contains("bigger than its record says"), "{refused}");

    // A record that doesn't say the size: nothing is fetched.
    store.state.lock().unwrap().serve_instead.clear();
    store.state.lock().unwrap().without_bytes = true;
    let packages = || store.state.lock().unwrap().requests.iter().filter(|(_, path, _)| path.ends_with("/package")).count();
    let before = packages();
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch", "{refused}");
    assert_eq!(packages(), before, "nothing fetched for a record without its size");

    // Nor for an update.
    store.state.lock().unwrap().without_bytes = false;
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    store.state.lock().unwrap().without_bytes = true;
    let before = packages();
    let checked = d.check().await;
    assert_eq!(checked["updates"], json!([]), "{checked}");
    assert!(word(checked["errors"][0]["error"].as_str().unwrap()) == "store_mismatch", "{checked}");
    assert_eq!(packages(), before);
}

#[tokio::test]
async fn a_hostile_store_s_words_reach_no_dialog_with_controls_or_direction_marks() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let hostile = "Focus\u{202e}draoB\u{1b}[2J\nok\u{200b}";
    store.state.lock().unwrap().card_extra = json!({
        "name": hostile, "summary": hostile, "description": hostile, "license": hostile,
        "author": { "username": hostile, "badges": [] }, "categories": [hostile], "keywords": [hostile],
    });
    let unseen = |text: &str| text.chars().any(zeron_engine::plugins::listing::unseen_char);
    let clean = |card: &Value| {
        ["name", "summary", "description", "license"].iter().all(|k| card[k].as_str().is_some_and(|t| !unseen(t) && t.contains("draoB")))
            && !unseen(card["author"]["username"].as_str().unwrap())
            && !unseen(card["categories"][0].as_str().unwrap())
            && !unseen(card["keywords"][0].as_str().unwrap())
    };
    let d = device(&store, Some("ada"));
    let found = d.call(methods::STORE_SEARCH, json!({})).await.unwrap();
    assert!(clean(&found["items"][0]), "{found}");
    let page = d.call(methods::STORE_PLUGIN, json!({ "id": "focus", "versions": true })).await.unwrap();
    assert!(clean(&page), "{page}");
    let mine = d.call(methods::STORE_MY_PLUGINS, json!({})).await.unwrap();
    assert!(clean(&mine["plugins"][0]), "{mine}");
}

#[tokio::test]
async fn a_store_answer_that_redirects_is_never_followed() {
    let store = FakeStore::start().await;
    let elsewhere = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let raw = package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({}));
    store.add("ada", &raw, "verified", "");
    elsewhere.add("ada", &raw, "verified", "");
    store.state.lock().unwrap().redirect_package = Some(format!("{}/store/v1/plugins/focus/versions/1.0.0/package", elsewhere.url));
    let d = device(&store, Some("bob"));
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_error", "{refused}");
    assert!(refused.contains("(302)"), "{refused}");
    assert!(elsewhere.state.lock().unwrap().requests.is_empty(), "the other origin was never asked");
    // (Without the redirect, it installs.)
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
}

#[tokio::test]
async fn a_package_that_isn_t_the_store_s_record_is_refused() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let real = package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({}));
    store.add("ada", &real, "verified", "");
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
        json!({ "id": "focus", "version": version, "hash": hash, "sha256": sha256(raw), "bytes": raw.len(), "key": key.public(), "publisher": publisher, "publisher_id": fake_store::publisher_id("ada"), "verified": true })
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
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
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
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
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
async fn an_agent_never_replaces_an_installed_store_plugin_or_opts_it_into_pre_releases() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let before = d.item("focus").await;
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    store.add("ada", &package_of(&key, "focus", "2.0.0-beta.1", &["ui.add"], "return 3", json!({})), "verified", "");
    let agent = || zeron_rpc::Caller::Agent { chat: Some("c1".into()) };

    // An agent's install over it, latest or a pre-release: refused, nothing changed.
    for params in [json!({ "store": "focus" }), json!({ "store": "focus", "version": "2.0.0-beta.1" })] {
        let refused = d.call_as(agent(), methods::INSTALL_PLUGIN, params.clone()).await.unwrap_err();
        assert_eq!(word(&refused), "needs_person", "{params}: {refused}");
        let now = d.item("focus").await;
        assert_eq!((now["version"].as_str(), now["hash"].as_str(), now["status"].as_str()), (Some("1.0.0"), before["hash"].as_str(), Some("on")));
        assert!(now.get("prerelease").is_none(), "an agent never opts the person into pre-releases");
    }
    // (Nothing was fetched for it.)
    let fetched = |v: &str| store.state.lock().unwrap().requests.iter().any(|(_, path, _)| path.ends_with(&format!("/{v}/package")));
    assert!(!fetched("1.1.0") && !fetched("2.0.0-beta.1"));

    // An agent's first install of another plugin still lands, waiting for
    // the person, and doesn't opt them in either.
    store.add("ada", &package_of(&key, "lab", "0.1.0-beta.1", &["ui.add"], "return 1", json!({})), "verified", "");
    let landed = d.call_as(agent(), methods::INSTALL_PLUGIN, json!({ "store": "lab", "version": "0.1.0-beta.1" })).await.unwrap();
    assert_eq!((landed["update"].as_bool(), landed["waiting"].as_bool()), (Some(false), Some(true)));
    assert!(d.item("lab").await.get("prerelease").is_none());

    // The person's own: from another of their devices (as the edge verified
    // the link), and from this one, the update and the pre-release go in.
    let peer = zeron_rpc::Caller::Peer { device: Some("dev-2".into()) };
    let updated = d.call_as(peer, methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.1.0" })).await.unwrap();
    assert_eq!((updated["update"].as_bool(), updated["plugin"]["version"].as_str()), (Some(true), Some("1.1.0")));
    let unverified = zeron_rpc::Caller::Peer { device: None };
    assert_eq!(word(&d.call_as(unverified, methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err()), "needs_person");
    let beta = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "2.0.0-beta.1" })).await.unwrap();
    assert_eq!(beta["plugin"]["version"], "2.0.0-beta.1");
    assert_eq!(d.item("focus").await["prerelease"], true, "the person's pre-release install takes its later ones");
}

#[tokio::test]
async fn an_agent_installs_only_what_zeron_has_verified() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "listed", "");
    store.add("eve", &package_of(&key, "fresh", "1.0.0", &["ui.add"], "return 1", json!({})), "listed", "");
    let d = device(&store, Some("bob"));
    let agent = || zeron_rpc::Caller::Agent { chat: Some("c1".into()) };
    // Without a version: the newest verified, not the newest.
    let installed = d.call_as(agent(), methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!((installed["plugin"]["version"].as_str(), installed["waiting"].as_bool()), (Some("1.0.0"), Some(true)), "{installed}");
    assert_eq!(installed["plugin"]["source"]["verified"], true);
    d.call(methods::REMOVE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    // Naming the unverified one, or a plugin with nothing verified (a new
    // one nobody has read, a squat): the person's, always.
    for params in [json!({ "store": "focus", "version": "1.1.0" }), json!({ "store": "fresh" }), json!({ "store": "fresh", "version": "1.0.0" })] {
        let refused = d.call_as(agent(), methods::INSTALL_PLUGIN, params.clone()).await.unwrap_err();
        assert_eq!(word(&refused), "needs_person", "{params}: {refused}");
    }
    assert!(d.item("focus").await.is_null() && d.item("fresh").await.is_null(), "nothing installed");
    // The person installs either, from its page.
    let theirs = d.call(methods::INSTALL_PLUGIN, json!({ "store": "fresh" })).await.unwrap();
    assert_eq!((theirs["plugin"]["source"]["verified"].as_bool(), theirs["waiting"].as_bool()), (Some(false), Some(true)));
}

#[tokio::test]
async fn take_downs_reach_the_device_by_the_feed_and_a_restore_asks_again() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    // No place in the feed yet: the full check says what's blocked, and the
    // device carries on from the head.
    d.plugins.check_takedowns().await;
    let asks = store.state.lock().unwrap().asks.len();
    assert_eq!(asks, 1, "a reset runs the full check once");
    // Taken down: the feed turns it off, with no full check.
    store.block("focus", "1.0.0", "It sent your chats somewhere.");
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("It sent your chats somewhere.")), "{item}");
    assert_eq!(store.state.lock().unwrap().asks.len(), asks, "nothing about what's installed was sent");
    let feed: Vec<String> = store.state.lock().unwrap().requests.iter().filter(|(_, p, a)| p.starts_with("/store/v1/takedowns") && a.is_none()).map(|(_, p, _)| p.clone()).collect();
    assert_eq!(feed.last().map(String::as_str), Some("/store/v1/takedowns?after=0"), "{feed:?}");
    // Restored (it comes back unverified): the block goes, and it asks again.
    store.restore("focus", "1.0.0");
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert!(item.get("blocked").is_none(), "{item}");
    assert_eq!((item["status"].as_str(), item["source"]["verified"].as_bool()), (Some("pending"), Some(false)), "{item}");
    // The device's place forgotten by the store: a reset, the full check again.
    store.state.lock().unwrap().feed_floor = 10;
    d.plugins.check_takedowns().await;
    assert_eq!(store.state.lock().unwrap().asks.len(), asks + 1);
}

/// A template's take-down is never a plugin's, whatever its id and version;
/// the device's place in the feed moves past it all the same.
#[tokio::test]
async fn a_template_s_take_down_never_touches_a_plugin() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    d.plugins.check_takedowns().await;
    {
        let mut state = store.state.lock().unwrap();
        state.events.push((1, "take_down".into(), "focus".into(), "1.0.0".into(), "A template's.".into()));
        state.template_events.insert(1);
    }
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item.get("blocked"), item["source"]["verified"].as_bool()), (Some("on"), None, Some(true)), "{item}");
    // The next event is the plugin's own, asked for after the template's.
    store.block("focus", "1.0.0", "Its own.");
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("Its own.")), "{item}");
    let asked = store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p == "/store/v1/takedowns?after=1").count();
    assert_eq!(asked, 1, "its place moved past the template's event");
}

#[tokio::test]
async fn a_featured_search_lists_only_the_featured_ones_in_the_store_s_order() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    for id in ["alpha", "beta", "gamma"] {
        store.add("ada", &package_of(&key, id, "1.0.0", &[], "return 1", json!({})), "verified", "");
    }
    store.feature("gamma");
    store.feature("alpha");
    let device = device(&store, Some("bob"));
    let ids = |reply: Value| reply["items"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    let featured = device.call(methods::STORE_SEARCH, json!({ "featured": true })).await.unwrap();
    assert_eq!(featured["total"], 2, "the store's total comes through");
    assert_eq!(ids(featured), ["alpha", "gamma"], "only the featured ones, in the order the store lists them");
    let path = store.state.lock().unwrap().requests.last().unwrap().1.clone();
    assert_eq!(path, "/store/v1/plugins?featured=true");
    let every = device.call(methods::STORE_SEARCH, json!({ "featured": false })).await.unwrap();
    assert_eq!(ids(every), ["alpha", "beta", "gamma"]);
    assert_eq!(store.state.lock().unwrap().requests.last().unwrap().1, "/store/v1/plugins", "false asks for no filter");
}

#[tokio::test]
async fn a_version_just_published_is_everyone_s_marked_by_its_own_record() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "listed", "");
    // Not verified yet, and anyone's: no version is the author's alone.
    let d = device(&store, Some("bob"));
    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!((installed["plugin"]["version"].as_str(), installed["plugin"]["author"]["username"].as_str()), (Some("1.0.0"), Some("ada")));
    assert_eq!(installed["waiting"], true, "installed from the store, it's asked about like anyone's");
    assert_eq!(installed["plugin"]["source"]["verified"], false);
    // The mark is the version's record's alone: a card or page that says
    // otherwise isn't read for it.
    d.call(methods::REMOVE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    store.state.lock().unwrap().card_extra = json!({ "verified": true, "verified_version": "1.0.0" });
    let again = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(again["plugin"]["source"]["verified"], false, "drawn from the version's record, never the card");
    // A reviewer verifies it: what's installed next is marked so.
    store.state.lock().unwrap().card_extra = Value::Null;
    store.verify("focus", "1.0.0");
    d.call(methods::REMOVE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    let verified = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(verified["plugin"]["source"]["verified"], true);
}

#[tokio::test]
async fn an_offer_the_device_can_t_read_hides_nothing_else() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    store.add("ada", &package_of(&key, "board", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let next = package_of(&key, "board", "1.1.0", &["ui.add"], "return 2", json!({}));
    store.add("ada", &next, "verified", "");
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
        state.offer_anyway.push(json!({ "id": "board", "version": "1.1.0", "hash": package.hash, "sha256": sha256(&next), "bytes": next.len(), "key": package.key, "publisher": "ada", "publisher_id": fake_store::publisher_id("ada"), "notes": null, "added": [], "removed": [], "verified": true }));
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
async fn reports_compatibility_for_another_zeron_and_deploy_tokens_for_owned_plugins_only() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({ "engines": { "zeron": ">=0.5.0" } })), "verified", "");
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

    // A token id that isn't one (`zeron plugin token revoke ..`): nothing sent.
    let sent = store.state.lock().unwrap().requests.len();
    for id in ["..", ".", "", "%2e%2e", "a/b"] {
        let refused = d.call(methods::REVOKE_DEPLOY_TOKEN, json!({ "id": id })).await.unwrap_err();
        assert!(refused.contains("isn't a deploy token's id"), "{id}: {refused}");
    }
    assert_eq!(store.state.lock().unwrap().requests.len(), sent);

    // bob has nothing in the store: no token yet; then, owning notes but no
    // focus, none for focus, said by id before any sign-in is asked for. A
    // scope that isn't one is refused.
    let refused = d.call(methods::CREATE_DEPLOY_TOKEN, json!({ "name": "CI", "scope": { "all": true } })).await.unwrap();
    assert_eq!(refused["error"], "not_published", "{refused}");
    // A template is something to publish too: owning only one, a token for
    // all of both kinds or for its id gets as far as the sign-in.
    store.state.lock().unwrap().templates.insert("midnight".into(), ("bob".into(), vec!["1.0.0".into()]));
    for scope in [json!({ "all": true, "all_kinds": ["plugin", "template"] }), json!({ "plugins": ["midnight"] })] {
        let refused = d.call(methods::CREATE_DEPLOY_TOKEN, json!({ "name": "CI", "scope": scope })).await.unwrap();
        assert_eq!(refused["error"], "step_up_required", "{scope}: {refused}");
    }
    // A store that has no templates route has none of theirs.
    store.state.lock().unwrap().without_templates = true;
    let refused = d.call(methods::CREATE_DEPLOY_TOKEN, json!({ "name": "CI", "scope": { "all": true } })).await.unwrap();
    assert_eq!(refused["error"], "not_published", "{refused}");
    store.state.lock().unwrap().without_templates = false;
    store.add("bob", &package_of(&key, "notes", "1.0.0", &[], "return 1", json!({})), "verified", "");
    let refused = d.call(methods::CREATE_DEPLOY_TOKEN, json!({ "name": "CI", "scope": { "plugins": ["focus", "board"] } })).await.unwrap();
    assert_eq!((refused["error"].as_str(), refused["ids"].clone()), (Some("not_owner"), json!(["board", "focus"])), "{refused}");
    for scope in [
        json!({}),
        json!({ "all": true, "plugins": ["focus"] }),
        json!({ "plugins": [] }),
        json!({ "plugins": ["notes"], "all_kinds": ["plugin"] }),
        json!({ "all": true, "all_kinds": ["template"] }),
        json!({ "all": true, "all_kinds": [] }),
    ] {
        let refused = d.call(methods::CREATE_DEPLOY_TOKEN, json!({ "name": "CI", "scope": scope })).await.unwrap();
        assert_eq!(refused["error"], "invalid_scope", "{scope}: {refused}");
    }
    // Without a step-up begun for it: refused, nothing made.
    let refused = d.call(methods::CREATE_DEPLOY_TOKEN, json!({ "name": "CI", "scope": { "plugins": ["notes"] } })).await.unwrap();
    assert_eq!(refused["error"], "step_up_required", "{refused}");
    assert!(store.state.lock().unwrap().deploy_tokens.is_empty());
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
    store.add("ada", &package_of(&key, "focus", "1.0.0", &[], "return 1", json!({})), "verified", "");
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
    store.add("ada", &package_of(&ada, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");

    store.move_username("focus", "someone-else");
    store.add("ada", &package_of(&other, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    let checked = d.check().await;
    assert_eq!(checked["updates"][0]["new_owner"], true, "{checked}");
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["plugin"]["author"]["username"], "ada", "the name people read is the same");
    assert_eq!(updated["plugin"]["status"], "pending", "but it's another account: asked again");
}

#[tokio::test]
async fn a_newer_version_supersedes_an_unverified_one_until_zeron_verifies_it() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "listed", "");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "listed", "");
    let d = device(&store, Some("bob"));
    // 1.0.0 was never read and a newer one is out: it takes no new installs.
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.0.0" })).await.unwrap_err();
    assert_eq!(word(&refused), "not_in_store", "{refused}");
    let page = d.call(methods::STORE_VERSION, json!({ "id": "focus", "version": "1.0.0" })).await.unwrap();
    assert_eq!((page["status"].as_str(), page["verified"].as_bool()), (Some("yanked"), Some(false)));
    // Why, for its page ("Replaced by 1.1.0 before it was verified").
    assert_eq!((page["yanked_by"].as_str(), page["superseded_by"].as_str()), (Some("superseded"), Some("1.1.0")), "{page}");
    let list = d.call(methods::STORE_VERSIONS, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(list["versions"][1]["superseded_by"], "1.1.0", "{list}");
    // Verified, it's installable again.
    store.verify("focus", "1.0.0");
    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.0.0" })).await.unwrap();
    assert_eq!((installed["plugin"]["version"].as_str(), installed["plugin"]["source"]["verified"].as_bool()), (Some("1.0.0"), Some(true)));
    // The author can't unyank a superseded version.
    let author = device(&store, Some("ada"));
    store.add("ada", &package_of(&key, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "listed", "");
    let unyank = author.call(methods::STORE_VERSION_ACTION, json!({ "id": "focus", "version": "1.1.0", "action": "unyank" })).await.unwrap();
    assert_eq!(unyank["ok"], false, "{unyank}");
}

#[tokio::test]
async fn every_publish_not_made_here_is_noticed_once_and_kept_until_the_app_has_shown_it() {
    let store = FakeStore::start().await;
    let (laptop, _) = SigningKey::generate().unwrap();
    store.add("ann", &package_of(&laptop, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("ann"));
    let notices = |list: Value| list["store_notices"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    // The first look: what was there before isn't news.
    d.plugins.check_publishes().await;
    assert!(notices(d.call(methods::LIST_PLUGINS, json!({})).await.unwrap()).is_empty());

    // A deploy token publishes 1.1.0 (unsigned); the laptop 1.2.0; this
    // device 1.3.0.
    let ci = store.add_deploy_token("ann", "CI", Scope::All { templates: false });
    let by_token = StoreClient::new(&store.url)
        .publish("focus", unsigned_package_of("focus", "1.1.0", &["ui.add"], "return 2"), &zeron_engine::plugins::store::client::Caller::DeployToken(Arc::new(DeployToken::parse(&ci).unwrap())))
        .await;
    assert!(by_token.is_ok(), "{by_token:?}");
    store.add("ann", &package_of(&laptop, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "listed", "");
    let here = zeron_engine::plugins::signing::load_or_create_key(d._dir.path()).unwrap();
    store.add("ann", &package_of(&here, "focus", "1.3.0", &["ui.add"], "return 4", json!({})), "listed", "");

    d.plugins.check_publishes().await;
    let list = d.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    assert_eq!(notices(list.clone()), ["focus@1.1.0", "focus@1.2.0"], "{list}");
    let shown = &list["store_notices"];
    assert_eq!((shown[0]["via"].as_str(), shown[0]["token_name"].as_str()), (Some("deploy_token"), Some("CI")), "{shown}");
    assert!(shown[0]["words"].as_str().unwrap().contains("your deploy token \"CI\""), "{shown}");
    assert_eq!(shown[1]["via"], "device");
    assert!(shown[1]["words"].as_str().unwrap().contains("another of your devices"), "{shown}");

    // Shown: let go, and never raised again.
    d.call(methods::ACK_STORE_NOTICES, json!({ "ids": ["focus@1.1.0"] })).await.unwrap();
    d.plugins.check_publishes().await;
    assert_eq!(notices(d.call(methods::LIST_PLUGINS, json!({})).await.unwrap()), ["focus@1.2.0"]);
    // Kept across a restart until it's shown.
    let again = Plugins::new(d._dir.path(), "dev-1");
    assert_eq!(notices(match again.dispatch(methods::LIST_PLUGINS, json!({})).await.unwrap() {
        RpcReply::Value(v) => v,
        RpcReply::Stream(_) => unreachable!(),
    }), ["focus@1.2.0"]);
}

#[tokio::test]
async fn a_record_that_doesn_t_say_verified_never_reads_as_verified() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    let agent = || zeron_rpc::Caller::Agent { chat: Some("c1".into()) };

    // The install record: refused as unreadable, for the person and an agent.
    store.state.lock().unwrap().records_without_verified = true;
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch", "{refused}");
    let refused = d.call_as(agent(), methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch", "{refused}");
    assert!(d.item("focus").await.is_null(), "nothing installed");

    // Installed from a good record; then the update's record, fetched again
    // as it's taken, doesn't say: the update is refused.
    store.state.lock().unwrap().records_without_verified = false;
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    assert_eq!(d.check().await["updates"][0]["version"], "1.1.0");
    store.state.lock().unwrap().records_without_verified = true;
    let refused = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch", "{refused}");
    assert_eq!(d.item("focus").await["version"], "1.0.0");
    store.state.lock().unwrap().records_without_verified = false;

    // An offer that doesn't say: not offered, said as an error.
    store.state.lock().unwrap().offers_without_verified = true;
    let checked = d.check().await;
    assert_eq!(checked["updates"], json!([]), "{checked}");
    assert!(checked["errors"].as_array().is_some_and(|e| e.iter().any(|e| e["id"] == "focus")), "{checked}");
    store.state.lock().unwrap().offers_without_verified = false;

    // An install whose .source.json doesn't say reads unverified, and its
    // row says so.
    let dir = std::path::PathBuf::from(d.item("focus").await["dir"].as_str().unwrap());
    let path = dir.join(".source.json");
    let mut source: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(source["store"]["verified"], true, "{source}");
    source["store"].as_object_mut().unwrap().remove("verified");
    std::fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let page = d.call(methods::STORE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(page["installed"]["verified"], false, "{page}");
    assert_eq!(d.item("focus").await["source"]["verified"], false);
}

#[tokio::test]
async fn a_first_install_s_sheet_offers_the_verified_version_instead_and_installs_it_by_its_record() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "- First.");
    // Verified, but for another Zeron: not one this device can offer.
    store.add("ada", &package_of(&key, "focus", "1.0.5", &["ui.add"], "return 2", json!({ "engines": { "zeron": ">=99.0.0" } })), "verified", "");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add", "links.open"], "return 3", json!({})), "listed", "");
    let d = device(&store, Some("bob"));
    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    let source = &installed["plugin"]["source"];
    assert_eq!(source["verified"], false, "{installed}");
    assert_eq!(source["verified_alternative"], json!({ "version": "1.0.0", "added": [], "removed": ["links.open"], "notes": "- First.", "verified_at": 1 }), "{installed}");
    let hash = installed["plugin"]["hash"].as_str().unwrap().to_owned();

    // Only the person, only for the item as shown.
    let agent = d.call_as(zeron_rpc::Caller::Agent { chat: None }, methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "focus", "hash": hash })).await;
    assert!(agent.is_err(), "{agent:?}");
    assert!(d.call(methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "focus", "hash": "0".repeat(64) })).await.is_err());
    // Its record no longer says verified when it's taken: refused.
    store.state.lock().unwrap().plugins.get_mut("focus").unwrap().versions[0].verified = false;
    let refused = d.call(methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "focus", "hash": hash })).await.unwrap_err();
    assert_eq!(word(&refused), "not_verified", "{refused}");
    assert_eq!(d.item("focus").await["version"], "1.1.0");
    store.verify("focus", "1.0.0");
    let instead = d.call(methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "focus", "hash": hash })).await.unwrap();
    assert_eq!((instead["plugin"]["version"].as_str(), instead["plugin"]["source"]["verified"].as_bool()), (Some("1.0.0"), Some(true)), "{instead}");
    assert!(instead["plugin"]["source"].get("verified_alternative").is_none());
}

#[tokio::test]
async fn an_update_s_sheet_installs_its_verified_alternative_by_the_engine_s_own_word() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    store.add("ada", &package_of(&key, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "listed", "");
    assert_eq!(d.check().await["updates"][0]["verified_alternative"]["version"], "1.1.0");
    let hash = d.item("focus").await["hash"].clone();
    let instead = d.call(methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "focus", "hash": hash })).await.unwrap();
    assert_eq!((instead["plugin"]["version"].as_str(), instead["plugin"]["source"]["verified"].as_bool()), (Some("1.1.0"), Some(true)), "{instead}");
}

#[tokio::test]
async fn with_the_store_unreachable_nothing_unverified_gets_through() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    // (a) Set to take verified updates: 1.1.0 is staged (its offer said
    // verified); with the store unreachable when it's taken, its record
    // can't say so: held.
    d.call(methods::SET_PLUGIN_UPDATES, json!({ "id": "focus", "prefer": "verified" })).await.unwrap();
    assert_eq!(d.check().await["updates"][0]["version"], "1.1.0");
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let held = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap_err();
    assert_eq!(word(&held), "not_verified", "{held}");
    // (b) An update-all doesn't take it either.
    d.call(methods::SET_PLUGIN_UPDATES, json!({ "id": "focus", "prefer": null })).await.unwrap();
    let bulk = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus", "verified_only": true })).await.unwrap_err();
    assert_eq!(word(&bulk), "not_verified", "{bulk}");
    // (c) An agent can do nothing with it: not update, not install over it,
    // not take a verified version instead.
    let agent = || zeron_rpc::Caller::Agent { chat: Some("c1".into()) };
    let hash = d.item("focus").await["hash"].clone();
    for (method, params) in [
        (methods::UPDATE_PLUGIN, json!({ "id": "focus" })),
        (methods::INSTALL_PLUGIN, json!({ "store": "focus", "version": "1.1.0" })),
        (methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "focus", "hash": hash })),
    ] {
        let refused = d.call_as(agent(), method, params).await.unwrap_err();
        assert!(matches!(word(&refused), "needs_person" | "review_needed") || refused.contains("person"), "{method}: {refused}");
    }
    assert_eq!(d.item("focus").await["version"], "1.0.0");
    // The person, on their own, takes it: marked not verified, and it asks.
    let taken = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!((taken["plugin"]["source"]["verified"].as_bool(), taken["plugin"]["status"].as_str()), (Some(false), Some("pending")), "{taken}");
}

#[tokio::test]
async fn a_plugin_transferred_in_brings_no_notice_of_its_former_owner_s_versions() {
    let store = FakeStore::start().await;
    let (laptop, _) = SigningKey::generate().unwrap();
    let (zeds, _) = SigningKey::generate().unwrap();
    store.add("ann", &package_of(&laptop, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("ann"));
    d.plugins.check_publishes().await;
    // Published by its former owner after this device last looked, then
    // transferred: not ann's publishes.
    store.add("zed", &package_of(&zeds, "moved", "1.0.0", &["ui.add"], "return 1", json!({})), "listed", "");
    store.transfer("moved", "ann");
    d.plugins.check_publishes().await;
    let ids = |list: Value| list["store_notices"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert!(ids(d.call(methods::LIST_PLUGINS, json!({})).await.unwrap()).is_empty());
    // Hers, from another device: noticed.
    store.add("ann", &package_of(&laptop, "moved", "1.1.0", &["ui.add"], "return 2", json!({})), "listed", "");
    d.plugins.check_publishes().await;
    assert_eq!(ids(d.call(methods::LIST_PLUGINS, json!({})).await.unwrap()), ["moved@1.1.0"]);
}

#[tokio::test]
async fn an_installed_version_becomes_verified_when_its_own_record_says_so() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "listed", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let verified_now = || async { d.item("focus").await["source"]["verified"] == true };
    let settle = || tokio::time::sleep(Duration::from_millis(300));

    // A record that names other content, or can't be read: nothing changes.
    {
        let mut state = store.state.lock().unwrap();
        let v = &mut state.plugins.get_mut("focus").unwrap().versions[0];
        v.verified = true;
        v.hash = "0".repeat(64);
    }
    d.check().await;
    settle().await;
    assert!(!verified_now().await, "another hash");
    let real = install::parse(store.state.lock().unwrap().plugins["focus"].versions[0].raw.clone()).unwrap().hash;
    store.state.lock().unwrap().plugins.get_mut("focus").unwrap().versions[0].hash = real;
    store.state.lock().unwrap().records_without_verified = true;
    d.check().await;
    settle().await;
    assert!(!verified_now().await, "unreadable");
    store.state.lock().unwrap().records_without_verified = false;

    // Staff verified it: the next check says so, and the row does.
    d.check().await;
    let mut became = false;
    for _ in 0..50 {
        if verified_now().await {
            became = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(became, "{}", d.item("focus").await);
    let item = d.item("focus").await;
    assert_eq!((item["source"]["verified_at"].as_i64(), item["status"].as_str()), (Some(1), Some("on")), "{item}");
    let page = d.call(methods::STORE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!(page["installed"]["verified"], true);

    // Never the reverse from a check: the record says not verified (a
    // take-down is the feed's), and the install record keeps its word.
    store.state.lock().unwrap().plugins.get_mut("focus").unwrap().versions[0].verified = false;
    d.check().await;
    settle().await;
    assert!(verified_now().await);

    // An agent gains nothing from it: it still can't install over it.
    let refused = d.call_as(zeron_rpc::Caller::Agent { chat: None }, methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "needs_person", "{refused}");
}

// ── The merged build's review (the contract's last section) ─────────────────

#[tokio::test]
async fn an_agent_never_replaces_a_store_plugin_from_a_file_nor_reinstalls_one() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let before = d.item("focus").await;
    let agent = || zeron_rpc::Caller::Agent { chat: Some("c1".into()) };
    // A newer version by the same key, as a file (the store's public
    // package route serves it, or the author's own release).
    let file = tempfile::tempdir().unwrap();
    let path = file.path().join("focus-1.1.0.zeron-plugin");
    std::fs::write(&path, package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({}))).unwrap();
    for params in [json!({ "path": path.display().to_string() }), json!({ "reinstall": "focus" })] {
        let refused = d.call_as(agent(), methods::INSTALL_PLUGIN, params.clone()).await.unwrap_err();
        assert_eq!(word(&refused), "needs_person", "{params}: {refused}");
        let now = d.item("focus").await;
        assert_eq!((now["version"].as_str(), now["hash"].as_str(), now["status"].as_str()), (Some("1.0.0"), before["hash"].as_str(), Some("on")), "{params}");
        assert_eq!(now["source"]["kind"], "store", "still the store's, and its take-downs reach it");
    }
    // The person reinstalls it.
    let again = d.call(methods::INSTALL_PLUGIN, json!({ "reinstall": "focus" })).await.unwrap();
    assert_eq!(again["plugin"]["version"], "1.0.0");
}

/// A device running each of `ids` (1.0.0, verified, from the store,
/// allowed), with its place in the take-down feed.
async fn running(ids: &[&str]) -> (FakeStore, SigningKey, Device) {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    for id in ids {
        store.add("ada", &package_of(&key, id, "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    }
    let d = device(&store, Some("bob"));
    for id in ids {
        d.call(methods::INSTALL_PLUGIN, json!({ "store": id })).await.unwrap();
        assert_eq!(d.allow(id).await["status"], "on");
        assert_eq!(d.item(id).await["source"]["verified"], true);
    }
    d.plugins.check_takedowns().await;
    (store, key, d)
}

#[tokio::test]
async fn taken_down_and_restored_in_one_page_comes_back_unmarked() {
    let (store, _key, d) = running(&["focus"]).await;
    store.block("focus", "1.0.0", "Bad.");
    store.restore("focus", "1.0.0");
    d.plugins.check_takedowns().await;
    assert_eq!(d.item("focus").await["source"]["verified"], false, "restored comes back unverified");
}

#[tokio::test]
async fn a_blocked_version_shows_no_mark_even_turned_back_on() {
    let (store, _key, d) = running(&["focus"]).await;
    store.block("focus", "1.0.0", "Bad.");
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["source"]["verified"].as_bool()), (Some("off"), Some(false)), "{item}");
    d.call(methods::SET_PLUGIN_OFF, json!({ "id": "focus", "off": false })).await.unwrap();
    assert_eq!(d.item("focus").await["source"]["verified"], false);
}

#[tokio::test]
async fn a_mark_ended_while_the_feed_forgot_its_place_ends_after_the_reset() {
    let (store, _key, d) = running(&["focus"]).await;
    store.block("focus", "1.0.0", "Bad.");
    store.restore("focus", "1.0.0");
    store.state.lock().unwrap().feed_floor = 1000;
    d.plugins.check_takedowns().await;
    assert_eq!(d.item("focus").await["source"]["verified"], false);
}

#[tokio::test]
async fn an_update_yanked_since_it_was_found_isn_t_taken() {
    let (store, key, d) = running(&["focus"]).await;
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    assert_eq!(d.check().await["updates"][0]["version"], "1.1.0");
    store.set_status("focus", "1.1.0", "yanked");
    let refused = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "not_in_store", "{refused}");
    assert_eq!(d.item("focus").await["version"], "1.0.0");
}

#[tokio::test]
async fn an_update_taken_down_since_it_was_found_isn_t_taken_with_the_store_unreachable() {
    let (store, key, d) = running(&["focus"]).await;
    store.add("ada", &package_of(&key, "focus", "1.1.0", &["ui.add"], "return 2", json!({})), "verified", "");
    assert_eq!(d.check().await["updates"][0]["version"], "1.1.0");
    store.block("focus", "1.1.0", "Bad.");
    d.plugins.check_takedowns().await;
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let refused = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(d.item("focus").await["version"], "1.0.0");
}

#[tokio::test]
async fn the_feed_s_place_never_passes_its_head() {
    let (store, _key, d) = running(&["focus"]).await;
    // An event far past the head the store gives: not a place to jump to.
    {
        let mut state = store.state.lock().unwrap();
        state.events.push((1_000_000, "take_down".into(), "elsewhere".into(), "1.0.0".into(), String::new()));
        state.feed_head = Some(0);
    }
    d.plugins.check_takedowns().await;
    {
        let mut state = store.state.lock().unwrap();
        state.events.clear();
        state.feed_head = None;
    }
    // So the next real take-down still reaches it.
    store.block("focus", "1.0.0", "Bad.");
    d.plugins.check_takedowns().await;
    assert_eq!(d.item("focus").await["status"], "off", "a take-down after the bogus event was missed");
}

#[tokio::test]
async fn a_block_the_device_can_t_read_keeps_the_block_it_had() {
    let (store, _key, d) = running(&["focus"]).await;
    store.block("focus", "1.0.0", "Bad.");
    d.plugins.check_takedowns().await;
    assert_eq!(d.item("focus").await["status"], "off");
    {
        let mut state = store.state.lock().unwrap();
        let v = state.plugins.get_mut("focus").unwrap().versions.iter_mut().find(|v| v.version == "1.0.0").unwrap();
        v.blocked = None;
        state.blocked_anyway = vec![json!({ "id": "focus", "version": 1 })];
    }
    d.check().await;
    let item = d.item("focus").await;
    assert_eq!(item["status"], "off", "{item}");
    assert!(item.get("blocked").is_some(), "{item}");
}

#[test]
fn a_version_is_at_most_32_characters() {
    use zeron_engine::plugins::version::Version;
    let longest = "1.0.0-aaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert_eq!(longest.len(), 32);
    assert!(Version::parse(longest).is_ok());
    assert!(Version::parse(&format!("{longest}a")).is_err());
    // And so the check before publishing refuses it (not_semver).
    let files: BTreeMap<String, Vec<u8>> = [("init.luau".to_string(), b"return 1".to_vec())].into();
    let (key, _) = SigningKey::generate().unwrap();
    let manifest = key.sign_manifest(&json!({ "id": "focus", "name": "Focus", "version": format!("{longest}a"), "entry": "init.luau" }), &package::hashes(&files));
    let manifest = zeron_engine::plugins::manifest::Manifest::from_value(manifest).unwrap();
    let (problems, _) = zeron_engine::plugins::listing::check(&manifest, &files);
    assert!(problems.iter().any(|p| p.rule == "not_semver"), "{problems:?}");
}

// ── Deploy tokens (contract a315eef5a) ──────────────────────────────────────

/// A version published with a deploy token is unsigned: installed from the
/// store, it's the publisher's (never "Unknown author"), trusted by the
/// record's hash as any store install is, on every scan; it updates to
/// another token's version or a device's; edited on disk, it's no longer
/// the store's. Unsigned is only ever the store's: never from a file, and
/// never for a version whose record names a signer.
#[tokio::test]
async fn a_version_published_with_a_token_installs_as_its_publisher_s() {
    let store = FakeStore::start().await;
    store.add("ada", &unsigned_package_of("focus", "1.0.0", &["ui.add"], "return 1"), "verified", "");
    let d = device(&store, Some("bob"));
    let installed = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    let ada = json!({ "kind": "verified", "username": "ada" });
    assert_eq!(installed["plugin"]["author"], ada, "{installed}");
    assert_eq!((installed["plugin"]["source"]["kind"].as_str(), installed["plugin"]["source"]["verified"].as_bool()), (Some("store"), Some(true)));
    assert_eq!(d.allow("focus").await["status"], "on");
    // Every scan after: still the publisher's; and `zeron plugin verify`
    // says so too (`store`), never "Unknown author".
    d.plugins.rescan().await;
    let item = d.item("focus").await;
    assert_eq!((item["author"].clone(), item["status"].as_str()), (ada.clone(), Some("on")), "{item}");
    let verified = d.call(methods::VERIFY_PLUGIN, json!({ "path": item["dir"] })).await.unwrap();
    assert_eq!((verified["status"].as_str(), verified["author"].clone()), (Some("store"), ada.clone()), "{verified}");

    // Another token's version, then a device's: updates as any.
    store.add("ada", &unsigned_package_of("focus", "1.1.0", &["ui.add"], "return 2"), "verified", "");
    assert_eq!(d.check().await["updates"][0]["version"], "1.1.0");
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!((updated["plugin"]["version"].as_str(), updated["plugin"]["author"].clone(), updated["plugin"]["status"].as_str()), (Some("1.1.0"), ada.clone(), Some("on")), "{updated}");
    let (desk, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&desk, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "verified", "");
    assert_eq!(d.check().await["updates"][0]["new_key"], Value::Null, "no key to have changed from");
    let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": "focus" })).await.unwrap();
    assert_eq!((updated["plugin"]["version"].as_str(), updated["plugin"]["author"].clone()), (Some("1.2.0"), ada.clone()), "{updated}");

    // Unsigned from a file: refused, as any unsigned sideload.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.zeron-plugin");
    std::fs::write(&file, unsigned_package_of("notes", "1.0.0", &[], "return 1")).unwrap();
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "path": file.display().to_string() })).await.unwrap_err();
    assert_eq!(word(&refused), "unsigned", "{refused}");
    // Unsigned bytes for a version whose record names a signer: refused.
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "board", "1.0.0", &[], "return 1", json!({})), "verified", "");
    store.state.lock().unwrap().serve_instead.insert(("board".into(), "1.0.0".into()), unsigned_package_of("board", "1.0.0", &[], "return 1"));
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "board" })).await.unwrap_err();
    assert!(matches!(word(&refused), "store_mismatch" | "unsigned"), "{refused}");
    assert!(d.item("board").await.is_null());

    // Unsigned, under a record that names a key: not the store's (it took
    // only a signed one), so whoever it is isn't known.
    store.add("ada", &unsigned_package_of("pad", "1.0.0", &["ui.add"], "return 1"), "verified", "");
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "pad" })).await.unwrap();
    assert_eq!(d.item("pad").await["author"], ada);
    let pad_dir = std::path::PathBuf::from(d.item("pad").await["dir"].as_str().unwrap());
    let source_path = pad_dir.join(install::SOURCE_FILE);
    let mut source: Value = serde_json::from_slice(&std::fs::read(&source_path).unwrap()).unwrap();
    source["store"]["key"] = json!(SigningKey::generate().unwrap().0.public());
    std::fs::write(&source_path, serde_json::to_vec(&source).unwrap()).unwrap();
    d.plugins.rescan().await;
    assert_eq!(d.item("pad").await["author"]["kind"], "unknown");

    // Edited on disk: no longer what the store vouched for.
    store.add("ada", &unsigned_package_of("lab", "1.0.0", &["ui.add"], "return 1"), "verified", "");
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "lab" })).await.unwrap();
    assert_eq!(d.allow("lab").await["status"], "on");
    let entry = d.item("lab").await["dir"].as_str().unwrap().to_owned();
    std::fs::write(std::path::Path::new(&entry).join("init.luau"), b"return 'changed'").unwrap();
    d.plugins.rescan().await;
    let item = d.item("lab").await;
    assert_eq!(item["author"]["kind"], "unknown", "{item}");
    assert_ne!(item["status"], "on", "{item}");
    let verified = d.call(methods::VERIFY_PLUGIN, json!({ "path": item["dir"] })).await.unwrap();
    assert_eq!((verified["status"].as_str(), verified["author"]["kind"].as_str()), (Some("unsigned"), Some("unknown")), "{verified}");
}

/// Under a record that names no signer, the store takes unsigned and
/// nothing else: a signed package there is refused; and an update kept
/// unsigned is refused when its record names a signer by the time it's
/// taken.
#[tokio::test]
async fn under_a_record_naming_no_signer_only_unsigned_is_taken() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    store.state.lock().unwrap().plugins.get_mut("focus").unwrap().versions[0].key = None;
    let d = device(&store, Some("bob"));
    let refused = d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch", "{refused}");
    assert!(d.item("focus").await.is_null());

    store.add("ada", &unsigned_package_of("board", "1.0.0", &["ui.add"], "return 1"), "verified", "");
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "board" })).await.unwrap();
    assert_eq!(d.allow("board").await["status"], "on");
    store.add("ada", &unsigned_package_of("board", "1.1.0", &["ui.add"], "return 2"), "verified", "");
    assert_eq!(d.check().await["updates"][0]["version"], "1.1.0");
    store.state.lock().unwrap().plugins.get_mut("board").unwrap().versions[1].key = Some(key.public());
    let refused = d.call(methods::UPDATE_PLUGIN, json!({ "id": "board" })).await.unwrap_err();
    assert_eq!(word(&refused), "store_mismatch", "{refused}");
    assert_eq!(d.item("board").await["version"], "1.0.0");
}

/// An unsigned version (a deploy token's) between two signed ones doesn't
/// hide a signing key change: the next signed update is held to the key
/// the last signed version had.
#[tokio::test]
async fn a_key_change_across_a_token_s_version_still_shows() {
    let store = FakeStore::start().await;
    let (first, _) = SigningKey::generate().unwrap();
    let (other, _) = SigningKey::generate().unwrap();
    let d = device(&store, Some("bob"));
    for id in ["focus", "board"] {
        store.add("ada", &package_of(&first, id, "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
        d.call(methods::INSTALL_PLUGIN, json!({ "store": id })).await.unwrap();
        assert_eq!(d.allow(id).await["status"], "on");
        store.add("ada", &unsigned_package_of(id, "1.1.0", &["ui.add"], "return 2"), "verified", "");
        d.check().await;
        let updated = d.call(methods::UPDATE_PLUGIN, json!({ "id": id })).await.unwrap();
        assert_eq!(updated["plugin"]["version"], "1.1.0", "{updated}");
    }
    store.add("ada", &package_of(&other, "focus", "1.2.0", &["ui.add"], "return 3", json!({})), "verified", "");
    store.add("ada", &package_of(&first, "board", "1.2.0", &["ui.add"], "return 3", json!({})), "verified", "");
    let checked = d.check().await;
    let offered = |id: &str| checked["updates"].as_array().unwrap().iter().find(|u| u["id"] == id).cloned().unwrap();
    assert_eq!(offered("focus")["new_key"], true, "{checked}");
    assert!(offered("board")["new_key"] != true, "{checked}");
}
