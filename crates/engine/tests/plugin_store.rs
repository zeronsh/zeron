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

/// A template taken down from the store: by the feed (with its contents'
/// hash) and by the full check; it and its copies can't be used, copied or
/// exported, nothing with its contents is imported, and a restore lifts it.
/// The device's place in the feed moves past it only once it's kept.
#[tokio::test]
async fn a_template_taken_down_is_blocked_here_and_so_are_its_copies() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    std::fs::write(templates.join(".zeron/app.luau"), "return 1").unwrap();
    // (Its folder's name is another's.)
    let folder = templates.join("midnight-2");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("zeron-template.json"), r#"{"id":"midnight","name":"Midnight","version":"1.0.0"}"#).unwrap();
    std::fs::write(folder.join("app.luau"), "return 2").unwrap();
    begin_blocks(&templates);
    let record = json!({ "key": null, "hash": "h", "version": "1.0.0", "source": { "kind": "store", "version": "1.0.0", "installed_at": 0,
        "store": { "id": "midnight", "publisher": "ada", "publisher_id": "pub_0123456789abcdef0123456789abcdef", "hash": "cd".repeat(32) } } });
    std::fs::write(folder.join(".imported.json"), record.to_string()).unwrap();
    d.call(methods::CREATE_TEMPLATE, json!({ "id": "mine", "from": "midnight-2" })).await.unwrap();
    d.call(methods::CREATE_TEMPLATE, json!({ "id": "again", "from": "mine" })).await.unwrap();
    let row = |list: &Value, id: &str| list["templates"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap().clone();

    // No place in the feed yet: the full check, which asks about it once.
    d.plugins.check_takedowns().await;
    let asks = store.state.lock().unwrap().asks.clone();
    assert_eq!((asks.len(), &asks[0]["templates"]), (1, &json!([{ "id": "midnight", "version": "1.0.0", "hash": "cd".repeat(32) }])), "{asks:?}");
    assert!(d.call(methods::USE_TEMPLATE, json!({ "id": "mine" })).await.is_ok());

    let down = |seq: u64, kind: &str| {
        let mut state = store.state.lock().unwrap();
        state.events.push((seq, kind.into(), "midnight".into(), "1.0.0".into(), "It read your chats.".into()));
        state.template_events.insert(seq);
    };
    down(1, "take_down");
    // It can't be kept: the place doesn't move, and it's asked for again.
    let file = templates.join(".blocked.json");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    d.plugins.check_takedowns().await;
    std::fs::remove_dir(&file).unwrap();
    // (What couldn't be read starts over: the full check, then the feed on.)
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.0.0".into()));
    d.plugins.check_takedowns().await;
    let list = d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap();
    for id in ["midnight-2", "mine", "again"] {
        let row = row(&list, id);
        assert_eq!((row["status"].as_str(), row["blocked"]["reason"].as_str()), (Some("blocked"), Some("From the full check.")), "{row}");
        for (method, params) in [
            (methods::USE_TEMPLATE, json!({ "id": id })),
            (methods::CREATE_TEMPLATE, json!({ "id": "another", "from": id })),
            (methods::EXPORT_TEMPLATE, json!({ "id": id })),
        ] {
            let refused = d.call(method, params).await.unwrap_err();
            assert!(refused.contains("taken_down"), "{method} {id}: {refused}");
        }
    }
    // A copy's row names what it's a copy of, and says it's a copy.
    let rows: Vec<Value> = ["midnight-2", "mine", "again"].iter().map(|id| row(&list, id)["blocked"].clone()).collect();
    for (blocked, via) in rows.iter().zip([Value::Null, json!("lineage"), json!("lineage")]) {
        assert_eq!((blocked["id"].as_str(), blocked["version"].as_str(), blocked["via"].clone()), (Some("midnight"), Some("1.0.0"), via), "{blocked}");
    }
    assert!(d.call(methods::USE_TEMPLATE, json!({ "id": "zeron" })).await.is_ok(), "Zeron's own always");

    // Restored, by the feed; then taken down again, by it.
    store.state.lock().unwrap().templates_down.clear();
    down(2, "restore");
    d.plugins.check_takedowns().await;
    assert!(d.call(methods::USE_TEMPLATE, json!({ "id": "again" })).await.is_ok());
    down(3, "take_down");
    d.plugins.check_takedowns().await;
    let list = d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap();
    assert_eq!(row(&list, "again")["blocked"]["reason"], "It read your chats.");
    let kept: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!((kept["format"].as_str(), kept["blocks"][0]["hash"].clone()), (Some("zeron-template-blocks:v1"), json!("ab".repeat(32))), "{kept}");

    let asked = |path: &str| store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p == path).count();
    assert!(asked("/store/v1/takedowns?after=2") >= 1);
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

/// The feed starts over only when the full check answered about every
/// template here: one it couldn't read leaves the place where it was.
#[tokio::test]
async fn a_reset_waits_for_the_templates_too() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join("midnight")).unwrap();
    std::fs::write(templates.join("midnight/zeron-template.json"), r#"{"id":"midnight","name":"Midnight"}"#).unwrap();
    let record = json!({ "key": null, "version": "1.0.0", "source": { "kind": "store", "version": "1.0.0", "installed_at": 0,
        "store": { "id": "midnight", "publisher": "ada", "publisher_id": "pub_0123456789abcdef0123456789abcdef", "hash": "cd".repeat(32) } } });
    std::fs::write(templates.join("midnight/.imported.json"), record.to_string()).unwrap();
    let starts = || store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p == "/store/v1/takedowns").count();
    // The check's answer about it can't be read.
    store.state.lock().unwrap().blocked_anyway.push(json!({ "id": "midnight", "item_kind": "template" }));
    d.plugins.check_takedowns().await;
    d.plugins.check_takedowns().await;
    assert_eq!(starts(), 2, "it starts over each time");
    // It can: the device takes its place.
    store.state.lock().unwrap().blocked_anyway.clear();
    d.plugins.check_takedowns().await;
    d.plugins.check_takedowns().await;
    assert_eq!(starts(), 3, "no more from the start");
}

/// A template and a copy of an earlier version of it are one id in two
/// versions: the store takes an id once a request, and a plugin's answer
/// never hangs on a template's. A lineage that isn't one is never sent.
#[tokio::test]
async fn a_copy_of_an_earlier_version_stops_no_check() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let templates = d._dir.path().join("templates");
    let folder = |id: &str, version: &str, lineage: Option<Value>| {
        let folder = templates.join(id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("zeron-template.json"), json!({ "id": id, "name": id }).to_string()).unwrap();
        match lineage {
            Some(lineage) => std::fs::write(folder.join(".lineage.json"), lineage.to_string()).unwrap(),
            None => {
                let record = json!({ "key": null, "version": version, "source": { "kind": "store", "version": version, "installed_at": 0,
                    "store": { "id": "midnight", "publisher": "ada", "publisher_id": "pub_0123456789abcdef0123456789abcdef", "hash": "cd".repeat(32) } } });
                std::fs::write(folder.join(".imported.json"), record.to_string()).unwrap();
            }
        }
    };
    begin_blocks(&templates);
    folder("midnight", "1.1.0", None);
    folder("mine", "", Some(json!([{ "id": "midnight", "version": "1.0.0", "hash": "ab".repeat(32) }])));
    folder("odd", "", Some(json!([{ "id": "Not An Id", "version": "1.0.0", "hash": "h" }])));
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.0.0".into()));
    store.block("focus", "1.0.0", "Its own.");
    let checked = d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
    assert_eq!(checked["errors"], json!([]), "{checked}");
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("Its own.")), "{item}");
    let asked: Vec<Vec<String>> = store.state.lock().unwrap().asks.iter().map(|a| a["templates"].as_array().into_iter().flatten().map(|t| t["version"].as_str().unwrap().to_owned()).collect()).collect();
    assert_eq!(asked, [vec!["1.0.0"], vec!["1.1.0"]], "each version of it, in a request of its own");
    // The copy of the version taken down is blocked; the newer one isn't.
    let kept: Value = serde_json::from_slice(&std::fs::read(templates.join(".blocked.json")).unwrap()).unwrap();
    assert_eq!((kept["blocks"].as_array().unwrap().len(), kept["blocks"][0]["version"].as_str()), (1, Some("1.0.0")), "{kept}");
    // And the feed takes its place (it had none).
    d.plugins.check_takedowns().await;
    d.plugins.check_takedowns().await;
    let starts = store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p == "/store/v1/takedowns").count();
    assert_eq!(starts, 1);
}

/// A template's take-down that can't be kept holds the place in the feed,
/// and nothing else: a plugin's is applied.
#[cfg(unix)]
#[tokio::test]
async fn a_template_s_block_that_cannot_be_kept_holds_up_no_plugin() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    d.plugins.check_takedowns().await;
    {
        let mut state = store.state.lock().unwrap();
        state.events.push((1, "take_down".into(), "midnight".into(), "1.0.0".into(), String::new()));
        state.template_events.insert(1);
    }
    store.block("focus", "1.0.0", "Its own.");
    // (Its folder can be read and not written.)
    use std::os::unix::fs::PermissionsExt as _;
    let mode = |mode| std::fs::set_permissions(&templates, std::fs::Permissions::from_mode(mode)).unwrap();
    mode(0o555);
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("Its own.")), "{item}");
    // The template is held to it all the same, unwritten.
    let written = || std::fs::read_to_string(templates.join(".blocked.json")).unwrap().contains("midnight");
    assert!(!written());
    let refused = d.call(methods::USE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap_err();
    assert!(refused.contains("taken_down"), "{refused}");
    mode(0o755);
    d.plugins.check_takedowns().await;
    assert!(written(), "asked for again, and kept");
    let after = |n: u64| store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| *p == format!("/store/v1/takedowns?after={n}")).count();
    assert_eq!((after(0), after(2)), (2, 0), "the place waited for it");
}

/// The block list, empty: there before anything from the store is.
fn begin_blocks(templates: &std::path::Path) {
    std::fs::create_dir_all(templates).unwrap();
    std::fs::write(templates.join(".blocked.json"), r#"{"format":"zeron-template-blocks:v1","blocks":[]}"#).unwrap();
}

/// Store template `id` at `version`, installed (no signer).
fn store_template(templates: &std::path::Path, id: &str, version: &str) {
    begin_blocks(templates);
    let folder = templates.join(id);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("zeron-template.json"), json!({ "id": id, "name": id }).to_string()).unwrap();
    let record = json!({ "key": null, "version": version, "source": { "kind": "store", "version": version, "installed_at": 0,
        "store": { "id": id, "publisher": "ada", "publisher_id": "pub_0123456789abcdef0123456789abcdef", "hash": "cd".repeat(32) } } });
    std::fs::write(folder.join(".imported.json"), record.to_string()).unwrap();
}

/// Plugins never stop because of templates: a store that refuses any check
/// that carries them still answers for the plugins, asked about alone; the
/// templates aren't taken as checked, and the feed doesn't take its place.
#[tokio::test]
async fn a_check_refused_for_its_templates_still_answers_for_the_plugins() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    store_template(&d._dir.path().join("templates"), "midnight", "1.0.0");
    store.state.lock().unwrap().refuse_template_asks = true;
    store.block("focus", "1.0.0", "Its own.");
    let checked = d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
    assert_eq!(checked["errors"], json!([]), "{checked}");
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("Its own.")), "{item}");
    let starts = || store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p == "/store/v1/takedowns").count();
    d.plugins.check_takedowns().await;
    d.plugins.check_takedowns().await;
    assert_eq!(starts(), 2, "the templates weren't checked: from the start each time");
    store.state.lock().unwrap().refuse_template_asks = false;
    d.plugins.check_takedowns().await;
    d.plugins.check_takedowns().await;
    assert_eq!(starts(), 3);
}

/// A take-down the feed brings while a full check is out isn't undone by
/// that check's older answer.
#[tokio::test]
async fn a_full_check_undoes_no_take_down_newer_than_its_question() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    d.plugins.check_takedowns().await;
    store.state.lock().unwrap().slow_updates = Some(std::time::Duration::from_millis(400));
    let full = d.call(methods::CHECK_PLUGIN_UPDATES, json!({}));
    let feed = async {
        // (The full check's answer is made by now, and on its way.)
        while store.state.lock().unwrap().asks.len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        {
            let mut state = store.state.lock().unwrap();
            state.events.push((1, "take_down".into(), "midnight".into(), "1.0.0".into(), "Newer.".into()));
            state.template_events.insert(1);
        }
        d.plugins.check_takedowns().await;
    };
    let (full, ()) = tokio::join!(full, feed);
    full.unwrap();
    let refused = d.call(methods::USE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap_err();
    assert!(refused.contains("taken_down"), "{refused}");
}

/// The block list deleted: it starts over by the full check, so what was
/// blocked before isn't lifted by the next event's list.
#[tokio::test]
async fn a_block_list_taken_away_starts_over() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    d.plugins.check_takedowns().await;
    let list = || -> Vec<String> {
        let kept: Value = serde_json::from_slice(&std::fs::read(templates.join(".blocked.json")).unwrap()).unwrap();
        kept["blocks"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap().to_owned()).collect()
    };
    assert!(list().is_empty(), "there, and empty");
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.0.0".into()));
    let event = |seq: u64, id: &str| {
        let mut state = store.state.lock().unwrap();
        state.events.push((seq, "take_down".into(), id.into(), "1.0.0".into(), String::new()));
        state.template_events.insert(seq);
    };
    event(1, "midnight");
    d.plugins.check_takedowns().await;
    assert_eq!(list(), ["midnight"]);
    std::fs::remove_file(templates.join(".blocked.json")).unwrap();
    event(2, "elsewhere");
    d.plugins.check_takedowns().await;
    assert!(list().contains(&"midnight".to_owned()), "{:?}", list());
}

/// A template's package, signed, as `id` at `version`.
fn template_package(key: &SigningKey, id: &str, version: &str, code: &str) -> Vec<u8> {
    let manifest = json!({ "id": id, "name": id, "description": "", "version": version, "zeron_version": "0.0.1" });
    let files = std::collections::BTreeMap::from([("app.luau".to_owned(), code.as_bytes().to_vec())]);
    zeron_engine::plugins::template_packages::pack(&manifest, &files, Some(key)).unwrap()
}

/// `raw` waiting as one from the store, by a record of `publisher_id`'s.
fn template_waits(d: &Device, raw: &[u8], publisher_id: &str, verified: bool) -> (String, String) {
    use zeron_engine::plugins::template_packages;
    let package = template_packages::parse(raw.to_vec()).unwrap();
    let at = d._dir.path().join("templates/.pending");
    std::fs::create_dir_all(&at).unwrap();
    let name = format!("{}-1", package.id());
    std::fs::write(at.join(format!("{name}.zeron-template")), raw).unwrap();
    let origin = json!({ "kind": "store", "version": package.version(), "installed_at": 0, "store": { "id": package.id(), "publisher": "ada", "publisher_id": publisher_id, "hash": package.content_hash, "key": package.key, "verified": verified } });
    std::fs::write(at.join(format!("{name}.json")), json!({ "pending": name, "id": package.id(), "key": package.key, "at": 1, "source": "store", "origin": origin }).to_string()).unwrap();
    (name, package.hash)
}

/// A store template is allowed under its record as the store has it now and
/// as the window draws it: any other answers what changed, at the review and
/// at the decision, and installs nothing.
#[tokio::test]
async fn a_store_template_is_allowed_as_its_record_is_now() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let raw = template_package(&key, "midnight", "1.0.0", "return 1");
    store.add_template("ada", &raw, "listed");
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    let ada = fake_store::publisher_id("ada");
    let (name, hash) = template_waits(&d, &raw, &ada, true);
    let shown = |verified: bool, publisher: &str, id: &str| json!({ "verified": verified, "publisher": publisher, "publisher_id": id, "version": "1.0.0" });
    let review = |shown: Value| d.call(methods::REVIEW_TEMPLATE, json!({ "pending": name, "hash": hash, "shown": shown }));
    let decide = |review: Value| d.call(methods::DECIDE_TEMPLATE, json!({ "pending": name, "allow": true, "hash": hash, "review": review["review"] }));
    let row = || async { d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap()["pending"][0].clone() };
    let set = |verified: bool| store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[0].verified = verified;

    // Drawn verified, as it waited: the store doesn't say so any more.
    let changed = review(shown(true, "ada", &ada)).await.unwrap();
    assert_eq!(changed, json!({ "changed": { "verified": false, "publisher": "ada", "publisher_id": ada } }));
    assert_eq!((&row().await["store"]["verified"], row().await["store"].get("verified_at")), (&json!(false), None), "the list says so too");
    // Drawn so now: changed again before the second Allow. Compared with
    // what's drawn, never with what was answered last.
    set(true);
    let changed = review(shown(false, "ada", &ada)).await.unwrap();
    assert_eq!((&changed["changed"]["verified"], &changed["changed"]["verified_at"]), (&json!(true), &json!(1)));
    set(false);
    assert_eq!(review(shown(true, "ada", &ada)).await.unwrap()["changed"]["verified"], false);
    // Its publisher, by name or by account.
    for drawn in [shown(false, "eve", &ada), shown(false, "ada", "pub_other")] {
        assert_eq!(review(drawn).await.unwrap()["changed"]["publisher_id"], json!(ada));
    }
    // As it is: a review, and between it and the decision it changes hands.
    let good = review(shown(false, "ada", &ada)).await.unwrap();
    store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().owner = "eve".into();
    let eve = fake_store::publisher_id("eve");
    assert_eq!(decide(good.clone()).await.unwrap(), json!({ "changed": { "verified": false, "publisher": "eve", "publisher_id": eve } }));
    assert!(!templates.join("midnight").exists() && row().await["store"]["publisher"] == "eve");
    assert!(decide(good).await.is_err(), "used up");
    // Nothing said of what's drawn, part of it, or another version or package: no review.
    for bad in [json!(null), json!({ "verified": false, "publisher": "eve", "publisher_id": eve }), json!({ "verified": false, "publisher": "eve", "publisher_id": eve, "version": "1.0.1" })] {
        assert!(review(bad.clone()).await.is_err(), "{bad}");
    }
    assert!(d.call(methods::REVIEW_TEMPLATE, json!({ "pending": name, "hash": "0".repeat(64), "shown": shown(false, "eve", &eve) })).await.is_err());
    // Allowed as it is: installed by the record as it is.
    let good = review(shown(false, "eve", &eve)).await.unwrap();
    let done = decide(good).await.unwrap();
    assert_eq!((done["installed"].as_str(), done["author"]["username"].as_str()), (Some("midnight"), Some("eve")), "{done}");
    let record: Value = serde_json::from_slice(&std::fs::read(templates.join("midnight/.imported.json")).unwrap()).unwrap();
    assert_eq!((&record["source"]["store"]["verified"], &record["source"]["store"]["publisher_id"]), (&json!(false), &json!(eve)));
}

/// One that can't be checked with the store isn't allowed, and waits; one
/// taken down, or no longer listed, says which.
#[tokio::test]
async fn a_store_template_that_cannot_be_checked_waits() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let raw = template_package(&key, "midnight", "1.0.0", "return 1");
    store.add_template("ada", &raw, "verified");
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    let ada = fake_store::publisher_id("ada");
    let (name, hash) = template_waits(&d, &raw, &ada, true);
    let shown = json!({ "verified": true, "publisher": "ada", "publisher_id": ada, "version": "1.0.0" });
    let review = || d.call(methods::REVIEW_TEMPLATE, json!({ "pending": name, "hash": hash, "shown": shown }));
    let decide = |review: Value| d.call(methods::DECIDE_TEMPLATE, json!({ "pending": name, "allow": true, "hash": hash, "review": review["review"] }));
    let offline = |on: bool| store.offline.store(on, std::sync::atomic::Ordering::SeqCst);
    let set = |status: &str| store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[0].status = status.to_owned();
    let words = "Zeron couldn't check this with the store. Try again.";
    offline(true);
    assert_eq!(review().await.unwrap_err(), words);
    offline(false);
    let good = review().await.unwrap();
    offline(true);
    assert_eq!(decide(good).await.unwrap_err(), words);
    offline(false);
    // A record that can't be read, or of something else.
    store.state.lock().unwrap().records_without_verified = true;
    assert!(review().await.unwrap_err().starts_with("store_mismatch: "));
    store.state.lock().unwrap().records_without_verified = false;
    let was = store.state.lock().unwrap().template_items["midnight"].versions[0].clone();
    for (hash, key) in [("0".repeat(64), was.key.clone()), (was.hash.clone(), None)] {
        {
            let mut state = store.state.lock().unwrap();
            let v = &mut state.template_items.get_mut("midnight").unwrap().versions[0];
            (v.hash, v.key) = (hash, key);
        }
        assert!(review().await.unwrap_err().starts_with("store_mismatch: "));
    }
    store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[0] = was;
    // (Another version is listed: it's this one that isn't.)
    store.add_template("ada", &template_package(&key, "midnight", "1.1.0", "return 2"), "listed");
    for status in ["yanked", "removed"] {
        set(status);
        assert!(review().await.unwrap_err().starts_with("not_in_store: "), "{status}");
    }
    set("taken_down");
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.0.0".into()));
    assert!(review().await.unwrap_err().starts_with("taken_down: "));
    // Known to be down here: without asking.
    set("listed");
    store.state.lock().unwrap().templates_down.clear();
    begin_blocks(&templates);
    let block = json!({ "id": "midnight", "version": "1.0.0", "reason": "", "at": 4_000_000_000u64 });
    std::fs::write(templates.join(".blocked.json"), json!({ "format": "zeron-template-blocks:v1", "blocks": [block] }).to_string()).unwrap();
    assert!(review().await.unwrap_err().starts_with("taken_down: "));
    begin_blocks(&templates);
    // All the while it waits, and is allowed once it can be checked.
    assert!(!templates.join("midnight").exists());
    let good = review().await.unwrap();
    assert_eq!(decide(good).await.unwrap()["installed"], "midnight");
}

/// A template from the store always waits for the person, whoever asked and
/// whatever its record says; it's counted once it's allowed.
#[tokio::test]
async fn a_template_from_the_store_always_waits() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add_template("bob", &template_package(&key, "midnight", "1.0.0", "return 1"), "verified");
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    // (bob's own, verified: it waits all the same.)
    let staged = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight" })).await.unwrap();
    assert_eq!((&staged["needs_consent"], &staged["id"], &staged["version"], &staged["store"]["verified"], &staged["from"]["kind"]), (&json!(true), &json!("midnight"), &json!("1.0.0"), &json!(true), &json!("store")), "{staged}");
    assert!(staged["digest"].is_string() && staged.get("update_of").is_none() && !templates.join("midnight").exists());
    assert!(!templates.join(".blocked.json").exists());
    let waiting = || async { d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap()["pending"].as_array().unwrap().clone() };
    assert_eq!(waiting().await.len(), 1);
    // Asked for again: the same one.
    let again = d.call(methods::IMPORT_TEMPLATE, json!({ "store": " midnight ", "version": "1.0.0" })).await.unwrap();
    assert_eq!((&again["pending"], waiting().await.len()), (&staged["pending"], 1));
    let installs = || store.state.lock().unwrap().template_items["midnight"].installs;
    assert_eq!(installs(), 0);
    let allowed = |staged: Value| {
        let d = &d;
        async move {
            let shown = json!({ "verified": staged["store"]["verified"], "publisher": staged["store"]["publisher"], "publisher_id": staged["store"]["publisher_id"], "version": staged["version"] });
            let review = d.call(methods::REVIEW_TEMPLATE, json!({ "pending": staged["pending"], "hash": staged["hash"], "shown": shown })).await?;
            d.call(methods::DECIDE_TEMPLATE, json!({ "pending": staged["pending"], "allow": true, "hash": staged["hash"], "review": review["review"] })).await
        }
    };
    let done = allowed(staged.clone()).await.unwrap();
    assert_eq!((done["installed"].as_str(), done["digest"] == staged["digest"]), (Some("midnight"), true), "{done}");
    for _ in 0..100 {
        if installs() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(installs(), 1);
    // The same version, or an older one: not over it, and nothing waits.
    for version in [None, Some("1.0.0")] {
        let refused = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight", "version": version })).await.unwrap_err();
        assert!(refused.starts_with("not_newer: "), "{refused}");
    }
    assert!(waiting().await.is_empty());
    // A newer one is an update of it, whatever folder it's in by now.
    store.add_template("bob", &template_package(&key, "midnight", "1.1.0", "return 2"), "listed");
    std::fs::rename(templates.join("midnight"), templates.join("mine")).unwrap();
    let update = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight", "version": "1.1.0" })).await.unwrap();
    assert_eq!((&update["update_of"], &update["store"]["verified"], update.get("new_owner")), (&json!("mine"), &json!(false), None), "{update}");
    assert_eq!(update["from"]["replaced"], json!({ "version": "1.0.0", "verified": true }));
    let done = allowed(update).await.unwrap();
    assert_eq!((done["updated"].as_str(), std::fs::read_to_string(templates.join("mine/app.luau")).unwrap().as_str()), (Some("mine"), "return 2"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(installs(), 1, "an update isn't a new install");
    // Beside one of its id from elsewhere, saying so.
    store.add_template("ada", &template_package(&key, "dusk", "1.0.0", "return 1"), "listed");
    std::fs::create_dir_all(templates.join("dusk")).unwrap();
    std::fs::write(templates.join("dusk/zeron-template.json"), r#"{"id":"dusk","name":"Dusk"}"#).unwrap();
    std::fs::write(templates.join("dusk/.imported.json"), json!({ "key": "k", "version": "0.1.0", "source": { "kind": "file", "version": "0.1.0", "installed_at": 0 } }).to_string()).unwrap();
    let beside = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "dusk" })).await.unwrap();
    assert_eq!((beside.get("update_of"), &beside["also_installed"]), (None, &json!([{ "id": "dusk", "from": { "kind": "file" } }])), "{beside}");
    assert_eq!(allowed(beside).await.unwrap()["installed"], "dusk-2");
}

/// A template's package with listing files of its own, its code `code`.
fn listed_template(key: &SigningKey, version: &str, code: &str, readme: &str) -> Vec<u8> {
    let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "", "version": version, "zeron_version": "0.0.1" });
    let files = BTreeMap::from([("app.luau".to_owned(), code.as_bytes().to_vec()), ("README.md".to_owned(), readme.as_bytes().to_vec())]);
    zeron_engine::plugins::template_packages::pack(&manifest, &files, Some(key)).unwrap()
}

/// `id` allowed on `d` as its window draws it.
async fn allow_template(d: &Device, staged: &Value) -> Result<Value, String> {
    let shown = json!({ "verified": staged["store"]["verified"], "publisher": staged["store"]["publisher"], "publisher_id": staged["store"]["publisher_id"], "version": staged["version"] });
    let review = d.call(methods::REVIEW_TEMPLATE, json!({ "pending": staged["pending"], "hash": staged["hash"], "shown": shown })).await?;
    d.call(methods::DECIDE_TEMPLATE, json!({ "pending": staged["pending"], "allow": true, "hash": staged["hash"], "review": review["review"] })).await
}

/// A device with store template midnight 1.0.0, verified, installed.
async fn with_midnight(store: &FakeStore, key: &SigningKey) -> Device {
    store.add_template("ada", &listed_template(key, "1.0.0", "return 1", "# One"), "verified");
    let d = device(store, Some("bob"));
    let zeron = d._dir.path().join("templates/.zeron");
    std::fs::create_dir_all(&zeron).unwrap();
    std::fs::write(zeron.join("zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    let staged = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight" })).await.unwrap();
    assert_eq!(allow_template(&d, &staged).await.unwrap()["installed"], "midnight");
    d
}

async fn template_at(d: &Device, id: &str) -> Value {
    d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap()["templates"].as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or(Value::Null)
}

/// A store template's update is the store's, said of its row and of its
/// page, taken by the person alone, and waits for them but for its listing
/// files, verified, over what wasn't edited.
#[tokio::test]
async fn a_store_template_is_updated_from_the_store() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let d = with_midnight(&store, &key).await;
    let folder = d._dir.path().join("templates/midnight");
    let check = || d.call(methods::CHECK_TEMPLATE_UPDATES, json!({}));
    let update = || d.call(methods::UPDATE_TEMPLATE, json!({ "id": "midnight" }));
    let page = || async { d.call(methods::STORE_PLUGIN, json!({ "id": "midnight", "kind": "template" })).await.unwrap()["installed"].clone() };
    assert_eq!(check().await.unwrap(), json!({ "updates": [], "errors": [] }));
    assert!(update().await.unwrap_err().starts_with("up_to_date: ") && page().await.get("update").is_none());

    // Its listing files alone, verified: found, said alike, installed in place.
    store.add_template("ada", &listed_template(&key, "1.0.1", "return 1", "# Two"), "verified");
    let found = check().await.unwrap()["updates"][0].clone();
    assert_eq!((&found["id"], &found["version"], &found["verified"], &found["installed_verified"], &found["notes"], found.get("new_owner")), (&json!("midnight"), &json!("1.0.1"), &json!(true), &json!(true), &json!("What's new."), None), "{found}");
    let row = template_at(&d, "midnight").await["update"].clone();
    assert_eq!((&row["version"], &row), (&json!("1.0.1"), &page().await["update"]), "the row and the page say one thing");
    let agent = d.call_as(zeron_rpc::Caller::Agent { chat: None }, methods::UPDATE_TEMPLATE, json!({ "id": "midnight" })).await;
    assert!(agent.unwrap_err().starts_with("needs_person: "));
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(update().await.unwrap_err(), "Zeron couldn't check this with the store. Try again.");
    store.offline.store(false, std::sync::atomic::Ordering::SeqCst);
    let done = update().await.unwrap();
    assert_eq!(done, json!({ "updated": "midnight", "name": "Midnight", "version": "1.0.1", "from": "1.0.0" }));
    let row = template_at(&d, "midnight").await;
    assert_eq!((&row["version"], &row["imported"]["source"]["verified"], row.get("update"), row.get("edited")), (&json!("1.0.1"), &json!(true), None, None), "{row}");
    assert_eq!(std::fs::read_to_string(folder.join("README.md")).unwrap(), "# Two");

    // The same, edited here; not verified any more; another publisher's; its
    // code: each waits.
    let mut n = 1;
    for what in ["edited", "unverified", "another's", "code"] {
        n += 1;
        let version = format!("1.0.{n}");
        let code = if what == "code" { "return 2" } else { "return 1" };
        store.add_template("ada", &listed_template(&key, &version, code, &format!("# {n}")), if what == "unverified" { "listed" } else { "verified" });
        match what {
            "edited" => std::fs::write(folder.join("README.md"), "# Mine").unwrap(),
            "another's" => store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().owner = "eve".into(),
            _ => {}
        }
        let found = check().await.unwrap()["updates"][0].clone();
        assert_eq!((&found["version"], found["new_owner"] == true, found["verified"] == true), (&json!(version), what == "another's", what != "unverified"), "{what}: {found}");
        let staged = update().await.unwrap();
        assert_eq!((&staged["needs_consent"], &staged["update_of"], &staged["replaces_edits"] == &json!(true), &staged["new_owner"] == &json!(true)), (&json!(true), &json!("midnight"), what == "edited", what == "another's"), "{what}: {staged}");
        assert_eq!(staged["from"]["replaced"]["version"], json!(format!("1.0.{}", n - 1)), "{what}");
        assert_eq!(update().await.unwrap()["pending"], staged["pending"], "{what}: asked again, the same one");
        assert!(template_at(&d, "midnight").await["update"].is_object(), "{what}: it's still found");
        let done = allow_template(&d, &staged).await.unwrap();
        assert_eq!((done["updated"].as_str(), template_at(&d, "midnight").await["version"].as_str().unwrap().to_owned()), (Some("midnight"), version), "{what}: {done}");
        assert!(template_at(&d, "midnight").await.get("update").is_none(), "{what}");
    }
    // Found verified and not any more as it's taken: it waits, as it is now.
    store.add_template("eve", &listed_template(&key, "1.1.0", "return 2", "# More"), "verified");
    check().await.unwrap();
    store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions.last_mut().unwrap().verified = false;
    let staged = update().await.unwrap();
    assert_eq!((&staged["needs_consent"], &staged["store"]["verified"]), (&json!(true), &json!(false)), "{staged}");
    // Gone, or taken down, by then: refused, saying which.
    let set = |status: &str| store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions.last_mut().unwrap().status = status.to_owned();
    set("yanked");
    assert!(update().await.unwrap_err().starts_with("not_in_store: "));
    set("taken_down");
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.1.0".into()));
    assert!(update().await.unwrap_err().starts_with("taken_down: "));
}

/// Which update a device takes of a store template: its own choice, else the
/// device's; pre-releases where it's said so; one found is asked for once.
#[tokio::test]
async fn a_store_template_s_update_is_the_one_chosen() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let d = with_midnight(&store, &key).await;
    let check = || async { d.call(methods::CHECK_TEMPLATE_UPDATES, json!({ "id": "midnight" })).await.unwrap() };
    store.add_template("ada", &listed_template(&key, "1.1.0", "return 2", "# Two"), "verified");
    store.add_template("ada", &listed_template(&key, "1.2.0", "return 3", "# Three"), "listed");
    store.add_template("ada", &listed_template(&key, "2.0.0-rc.1", "return 4", "# Four"), "listed");
    let found = check().await["updates"][0].clone();
    assert_eq!((&found["version"], &found["verified"], &found["verified_alternative"]["version"]), (&json!("1.2.0"), &json!(false), &json!("1.1.0")), "{found}");
    let packages = || store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p.ends_with("/package")).count();
    let before = packages();
    check().await;
    assert_eq!(packages(), before, "the one kept is read, not fetched again");
    d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": "verified" })).await.unwrap();
    assert_eq!(check().await["updates"][0]["version"], "1.1.0");
    // (Asked to be taken after: verified alone is, as its record says now.)
    store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[1].verified = false;
    assert!(d.call(methods::UPDATE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap_err().starts_with("not_verified: "));
    let held = check().await["updates"][0].clone();
    assert_eq!((&held["held"], &held["version"], &held["verified"], &held["installed_verified"]), (&json!(true), &json!("1.2.0"), &json!(false), &json!(true)), "{held}");
    let said = json!({ "version": "1.2.0", "verified": false, "held": true, "installed_verified": true });
    let page = d.call(methods::STORE_PLUGIN, json!({ "id": "midnight", "kind": "template" })).await.unwrap()["installed"]["update"].clone();
    assert_eq!((&template_at(&d, "midnight").await["update"], &page), (&said, &said), "held, its row and its page say so");
    assert!(!d._dir.path().join("templates/.updates/midnight.zeron-template").exists(), "its package isn't kept");
    assert!(d.call(methods::UPDATE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap_err().starts_with("not_verified: "));
    d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": null })).await.unwrap();
    d.call(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": true })).await.unwrap();
    assert_eq!(check().await["updates"][0]["version"], "2.0.0-rc.1");
    // An offer that doesn't say whether it's verified isn't taken for one.
    store.state.lock().unwrap().offers_without_verified = true;
    let odd = check().await;
    assert!(odd["updates"].as_array().unwrap().is_empty() && odd["errors"][0]["id"] == "midnight", "{odd}");
    store.state.lock().unwrap().offers_without_verified = false;
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(check().await["errors"][0]["id"], "midnight");
    store.offline.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(check().await["errors"], json!([]));
    // Of another template, nothing's asked.
    let asks = || store.state.lock().unwrap().asks.len();
    let before = asks();
    d.call(methods::CHECK_TEMPLATE_UPDATES, json!({ "id": "other" })).await.unwrap();
    assert_eq!(asks(), before);
}

/// What was found by another choice isn't what's taken: one that changes what
/// a template takes forgets what was found of it, kept or being looked for.
#[tokio::test]
async fn a_changed_choice_forgets_what_was_found_by_the_other() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let d = with_midnight(&store, &key).await;
    let check = || d.call(methods::CHECK_TEMPLATE_UPDATES, json!({ "id": "midnight" }));
    let set = |method: &'static str, params: Value| d.call(method, params);
    let kept = async || template_at(&d, "midnight").await["update"]["version"].clone();
    let update = || d.call(methods::UPDATE_TEMPLATE, json!({ "id": "midnight" }));
    store.add_template("ada", &listed_template(&key, "1.1.0", "return 2", "# Two"), "listed");
    store.add_template("ada", &listed_template(&key, "2.0.0-rc.1", "return 4", "# Four"), "listed");
    set(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": true })).await.unwrap();
    check().await.unwrap();
    assert_eq!(kept().await, "2.0.0-rc.1");
    // The same again, or what changes nothing of it, forgets nothing.
    set(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": true })).await.unwrap();
    set(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": "newest" })).await.unwrap();
    set(methods::SET_PLUGIN_UPDATES, json!({ "prefer": "verified" })).await.unwrap();
    assert_eq!(kept().await, "2.0.0-rc.1", "its own choice is over the device's");
    // Off: the pre-release found isn't brought.
    set(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": false })).await.unwrap();
    let updates = d._dir.path().join("templates/.updates");
    assert_eq!((kept().await, updates.join("midnight.zeron-template").exists(), updates.join("midnight.json").exists()), (Value::Null, false, false));
    assert_eq!(update().await.unwrap()["version"], "1.1.0");
    // As the device does, which takes verified ones: held; then it takes any.
    set(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": null })).await.unwrap();
    assert!(update().await.unwrap_err().starts_with("not_verified: "));
    assert_eq!(template_at(&d, "midnight").await["update"]["held"], true);
    set(methods::SET_PLUGIN_UPDATES, json!({ "prefer": "newest" })).await.unwrap();
    assert_eq!(kept().await, Value::Null);
    assert_eq!(update().await.unwrap()["version"], "1.1.0");
    set(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": "verified" })).await.unwrap();
    // Being looked for as it's chosen: not kept.
    for (asked, next) in [("newest", "verified"), ("verified", "newest")] {
        set(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": asked })).await.unwrap();
        store.state.lock().unwrap().slow_updates = Some(std::time::Duration::from_secs(1));
        let asks = store.state.lock().unwrap().asks.len();
        let chosen = async {
            while store.state.lock().unwrap().asks.len() == asks {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            set(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": next })).await.unwrap();
        };
        let (checked, ()) = tokio::join!(check(), chosen);
        store.state.lock().unwrap().slow_updates = None;
        let checked = checked.unwrap();
        assert_eq!((&checked["updates"], &checked["errors"][0]["id"]), (&json!([]), &json!("midnight")), "{asked}: {checked}");
        assert_eq!(kept().await, Value::Null, "{asked}");
    }
}

/// The verified version instead, over one that's here: newer than a verified
/// one; or, over one that isn't, the newest verified though it's older, by
/// the person's own choice alone, and over the very one its window names.
#[tokio::test]
async fn an_older_verified_version_goes_over_an_unverified_one_by_choice() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let d = with_midnight(&store, &key).await;
    let import = |version: &str| d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight", "version": version }));
    let instead = |staged: &Value| d.call(methods::INSTALL_VERIFIED_TEMPLATE, json!({ "pending": staged["pending"], "hash": staged["hash"] }));
    let decline = |staged: &Value| d.call(methods::DECIDE_TEMPLATE, json!({ "pending": staged["pending"], "allow": false }));
    let version = || async { template_at(&d, "midnight").await["version"].clone() };
    // Over the newest verified one itself: there's none to offer.
    store.add_template("ada", &listed_template(&key, "1.1.0", "return 2", "# Two"), "listed");
    let staged = import("1.1.0").await.unwrap();
    assert_eq!((&staged["store"]["verified_alternative"], &staged["update_of"]), (&Value::Null, &json!("midnight")), "{staged}");
    assert!(instead(&staged).await.unwrap_err().starts_with("not_verified: "));
    allow_template(&d, &staged).await.unwrap();
    assert_eq!(version().await, "1.1.0");
    // Over an unverified one, the newest verified is offered, older as it is,
    // and never one older than it.
    store.add_template("ada", &listed_template(&key, "0.9.0", "return 0", "# Nought"), "verified");
    store.add_template("ada", &listed_template(&key, "1.2.0", "return 3", "# Three"), "listed");
    let update = d.call(methods::UPDATE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap();
    assert_eq!(update["store"]["verified_alternative"]["version"], "1.0.0", "{update}");
    decline(&update).await.unwrap();
    let staged = import("1.2.0").await.unwrap();
    assert_eq!(staged["store"]["verified_alternative"]["version"], "1.0.0", "{staged}");
    decline(&staged).await.unwrap();
    // A verified pre-release only where this device takes them of it,
    // whatever version was asked for.
    store.add_template("ada", &listed_template(&key, "1.1.5-rc.1", "return 4", "# Rc"), "verified");
    store.add_template("ada", &listed_template(&key, "1.1.6-rc.2", "return 5", "# Rc"), "listed");
    let alternative = |staged: Value| staged["store"]["verified_alternative"]["version"].clone();
    for (asked, takes, expected) in [("1.2.0", false, "1.0.0"), ("1.2.0", true, "1.1.5-rc.1"), ("1.1.6-rc.2", false, "1.0.0"), ("1.1.6-rc.2", true, "1.1.5-rc.1")] {
        d.call(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": takes })).await.unwrap();
        let staged = import(asked).await.unwrap();
        decline(&staged).await.unwrap();
        assert_eq!(alternative(staged), json!(expected), "{asked}, {takes}");
    }
    d.call(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": false })).await.unwrap();
    let staged = import("1.2.0").await.unwrap();
    // Never any other way: not by its version, an agent's, or an update.
    for older in ["1.0.0", "0.9.0"] {
        assert!(import(older).await.unwrap_err().starts_with("not_newer: "), "{older}");
        let agent = d.call_as(zeron_rpc::Caller::Agent { chat: None }, methods::IMPORT_TEMPLATE, json!({ "store": "midnight", "version": older })).await;
        assert!(agent.unwrap_err().starts_with("not_newer: "), "{older}");
    }
    let chosen = instead(&staged).await.unwrap();
    assert_eq!((&chosen["version"], &chosen["instead"], &chosen["update_of"], &chosen["from"]["replaced"]), (&json!("1.0.0"), &json!(true), &json!("midnight"), &json!({ "version": "1.1.0", "verified": false })), "{chosen}");
    // Edited since: its window says that too.
    let app = d._dir.path().join("templates/midnight/app.luau");
    let code = std::fs::read(&app).unwrap();
    std::fs::write(&app, "mine").unwrap();
    let waiting = d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap()["pending"].as_array().unwrap().iter().find(|p| p["pending"] == chosen["pending"]).cloned().unwrap();
    assert_eq!((&waiting["replaces_edits"], &waiting["instead"]), (&json!(true), &json!(true)), "{waiting}");
    std::fs::write(&app, code).unwrap();
    // What's here isn't what its window says it replaces any more: not over it.
    let record = d._dir.path().join("templates/midnight/.imported.json");
    let was = std::fs::read_to_string(&record).unwrap();
    for (pointer, value) in [("/version", json!("1.1.5")), ("/source/store/verified", json!(true))] {
        let mut now: Value = serde_json::from_str(&was).unwrap();
        *now.pointer_mut(pointer).unwrap() = value;
        std::fs::write(&record, now.to_string()).unwrap();
        assert!(allow_template(&d, &chosen).await.unwrap_err().starts_with("not_newer: "), "{pointer}");
    }
    std::fs::write(&record, &was).unwrap();
    // Nor when the store doesn't say it's verified by then.
    let set = |verified: bool| store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[0].verified = verified;
    set(false);
    let changed = allow_template(&d, &chosen).await;
    assert!(changed.is_err(), "{changed:?}");
    let mut unverified = chosen.clone();
    unverified["store"]["verified"] = json!(false);
    assert!(allow_template(&d, &unverified).await.unwrap_err().starts_with("not_newer: "));
    set(true);
    let _ = allow_template(&d, &unverified).await;
    assert_eq!(version().await, "1.1.0");
    let done = allow_template(&d, &chosen).await.unwrap();
    assert_eq!((done["updated"].as_str(), version().await), (Some("midnight"), json!("1.0.0")), "{done}");
    // A verified one is here now: nothing older goes over it, by any choice.
    let staged = import("1.2.0").await.unwrap();
    assert_eq!(staged["store"]["verified_alternative"], Value::Null, "{staged}");
    decline(&staged).await.unwrap();
    // Build metadata is no newer version.
    store.add_template("ada", &listed_template(&key, "1.0.0+b", "return 1", "# One"), "verified");
    assert!(import("1.0.0+b").await.unwrap_err().starts_with("not_newer: "));
}

/// Two versions of one id waiting before either is allowed: the second isn't
/// put beside the first.
#[tokio::test]
async fn two_that_wait_are_never_two_of_one_template() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add_template("ada", &listed_template(&key, "1.0.0", "return 1", "# One"), "verified");
    store.add_template("ada", &listed_template(&key, "1.1.0", "return 2", "# Two"), "verified");
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    let import = |version: &str| d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight", "version": version }));
    let (first, second) = (import("1.0.0").await.unwrap(), import("1.1.0").await.unwrap());
    assert!(first["pending"] != second["pending"] && second.get("update_of").is_none());
    assert_eq!(allow_template(&d, &first).await.unwrap()["installed"], "midnight");
    assert!(allow_template(&d, &second).await.unwrap_err().contains("Ask for it again"));
    assert!(!templates.join("midnight-2").exists());
    let update = import("1.1.0").await.unwrap();
    assert_eq!((&update["update_of"], update["pending"] != second["pending"]), (&json!("midnight"), true), "{update}");
    assert_eq!(allow_template(&d, &update).await.unwrap()["updated"], "midnight");
}

/// What a template is refused for as it's fetched, and that nothing waits.
#[tokio::test]
async fn a_template_from_the_store_is_refused_as_its_record_says() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let raw = template_package(&key, "midnight", "1.0.0", "return 1");
    store.add_template("ada", &raw, "listed");
    let later = json!({ "id": "later", "name": "Later", "description": "", "version": "1.0.0", "zeron_version": "9999.0.0" });
    let files = BTreeMap::from([("app.luau".to_owned(), b"x".to_vec())]);
    store.add_template("ada", &zeron_engine::plugins::template_packages::pack(&later, &files, Some(&key)).unwrap(), "verified");
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    let import = |params: Value| d.call(methods::IMPORT_TEMPLATE, params);
    let agent = |params: Value| d.call_as(zeron_rpc::Caller::Agent { chat: Some("c1".into()) }, methods::IMPORT_TEMPLATE, params);
    // An agent's is verified, or the person's to ask for.
    for params in [json!({ "store": "midnight" }), json!({ "store": "midnight", "version": "1.0.0" })] {
        assert!(agent(params.clone()).await.unwrap_err().starts_with("needs_person: "), "{params}");
    }
    assert!(import(json!({ "store": "later" })).await.unwrap_err().starts_with("incompatible: "));
    assert!(import(json!({ "store": "nothing-here" })).await.unwrap_err().starts_with("not_in_store: "));
    for params in [json!({ "store": "Midnight" }), json!({ "store": "" }), json!({ "store": 1 }), json!({ "store": "midnight", "version": "1.0" }), json!({ "store": "midnight", "path": "/x" }), json!({ "store": "midnight", "url": "https://x" })] {
        assert!(import(params.clone()).await.is_err(), "{params}");
    }
    // Other bytes than the record's, another id's, a yanked version, one
    // taken down here.
    store.state.lock().unwrap().serve_instead.insert(("midnight".into(), "1.0.0".into()), template_package(&key, "midnight", "1.0.0", "return 'other'"));
    assert!(import(json!({ "store": "midnight" })).await.unwrap_err().starts_with("store_mismatch: "));
    store.state.lock().unwrap().records_without_verified = true;
    assert!(import(json!({ "store": "midnight" })).await.unwrap_err().starts_with("store_mismatch: "));
    store.state.lock().unwrap().records_without_verified = false;
    store.add_template("ada", &template_package(&key, "midnight", "1.1.0", "return 2"), "yanked");
    assert!(import(json!({ "store": "midnight", "version": "1.1.0" })).await.unwrap_err().starts_with("not_in_store: "));
    let hash = store.state.lock().unwrap().template_items["midnight"].versions[0].hash.clone();
    std::fs::write(templates.join(".blocked.json"), json!({ "format": "zeron-template-blocks:v1", "blocks": [{ "id": "elsewhere", "version": "9.9.9", "hash": hash, "reason": "Bad.", "at": 4_000_000_000u64 }] }).to_string()).unwrap();
    assert!(import(json!({ "store": "midnight" })).await.unwrap_err().starts_with("taken_down: "), "by its contents");
    begin_blocks(&templates);
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(import(json!({ "store": "midnight" })).await.is_err());
    store.offline.store(false, std::sync::atomic::Ordering::SeqCst);
    let list = d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap();
    assert!(list["pending"].as_array().unwrap().is_empty() && list["templates"].as_array().unwrap().len() == 1, "{list}");
    // Verified, an agent's waits too, as anyone's.
    store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[0].verified = true;
    let staged = agent(json!({ "store": "midnight" })).await.unwrap();
    assert_eq!((&staged["needs_consent"], templates.join("midnight").exists()), (&json!(true), false));
}

/// The verified version a waiting template's sheet offers: its own item's,
/// its record read now, waiting in its place; the person's to choose.
#[tokio::test]
async fn the_verified_version_instead_waits_in_its_place() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add_template("ada", &template_package(&key, "midnight", "1.0.0", "return 1"), "verified");
    store.add_template("ada", &template_package(&key, "midnight", "1.1.0", "return 2"), "listed");
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    // (By its own record, not its pages' word.)
    let records = |unverified: bool| store.state.lock().unwrap().records_unverified = unverified;
    records(true);
    let none = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight" })).await.unwrap();
    assert_eq!(none["store"]["verified_alternative"], Value::Null, "{none}");
    d.call(methods::DECIDE_TEMPLATE, json!({ "pending": none["pending"], "allow": false })).await.unwrap();
    records(false);
    let staged = d.call(methods::IMPORT_TEMPLATE, json!({ "store": "midnight" })).await.unwrap();
    assert_eq!((&staged["version"], &staged["store"]["verified"], &staged["store"]["verified_alternative"]["version"]), (&json!("1.1.0"), &json!(false), &json!("1.0.0")), "{staged}");
    // Verified since, and known to be: there's nothing to choose instead.
    let newest = |verified: bool| store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[1].verified = verified;
    let shown = |verified: bool| json!({ "verified": verified, "publisher": staged["store"]["publisher"], "publisher_id": staged["store"]["publisher_id"], "version": "1.1.0" });
    let review = |verified: bool| d.call(methods::REVIEW_TEMPLATE, json!({ "pending": staged["pending"], "hash": staged["hash"], "shown": shown(verified) }));
    newest(true);
    assert_eq!(review(false).await.unwrap()["changed"]["verified"], true);
    assert!(d.call(methods::INSTALL_VERIFIED_TEMPLATE, json!({ "pending": staged["pending"], "hash": staged["hash"] })).await.unwrap_err().starts_with("not_verified: "));
    newest(false);
    assert_eq!(review(true).await.unwrap()["changed"]["verified"], false);
    let instead = |staged: &Value| json!({ "pending": staged["pending"], "hash": staged["hash"] });
    let agent = d.call_as(zeron_rpc::Caller::Agent { chat: None }, methods::INSTALL_VERIFIED_TEMPLATE, instead(&staged)).await;
    assert!(agent.unwrap_err().starts_with("needs_person: "));
    assert!(d.call(methods::INSTALL_VERIFIED_TEMPLATE, json!({ "pending": staged["pending"], "hash": "0" })).await.is_err());
    assert!(d.call(methods::INSTALL_VERIFIED_TEMPLATE, json!({ "pending": "none-1", "hash": staged["hash"] })).await.is_err());
    // Its record doesn't say so any more: refused, and it waits as it was.
    let set = |verified: bool| store.state.lock().unwrap().template_items.get_mut("midnight").unwrap().versions[0].verified = verified;
    set(false);
    assert!(d.call(methods::INSTALL_VERIFIED_TEMPLATE, instead(&staged)).await.unwrap_err().starts_with("not_verified: "));
    let pending = || async { d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap()["pending"].as_array().unwrap().clone() };
    assert_eq!(pending().await[0]["pending"], staged["pending"]);
    set(true);
    let verified = d.call(methods::INSTALL_VERIFIED_TEMPLATE, instead(&staged)).await.unwrap();
    assert_eq!((&verified["version"], &verified["store"]["verified"], &verified["store"]["verified_alternative"], &verified["needs_consent"]), (&json!("1.0.0"), &json!(true), &Value::Null, &json!(true)), "{verified}");
    assert_eq!((pending().await.len(), pending().await[0]["pending"] == verified["pending"], templates.join("midnight").exists()), (1, true, false));
    assert!(d.call(methods::INSTALL_VERIFIED_TEMPLATE, instead(&verified)).await.unwrap_err().starts_with("not_verified: "), "it's the verified one");
}

/// A call that names a kind asks under its own routes and answers what it's
/// of; one that names none is a plugin's, as it was; any other kind is
/// refused, asking nothing.
#[tokio::test]
async fn the_store_s_calls_are_of_one_kind() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    store.add_template("ada", &template_package(&key, "midnight", "1.0.0", "return 1"), "listed");
    store.add_template("ada", &template_package(&key, "midnight", "1.1.0", "return 2"), "listed");
    let d = device(&store, Some("ada"));
    let calls = [
        (methods::STORE_SEARCH, json!({}), "/store/v1/templates"),
        (methods::STORE_PLUGIN, json!({ "id": "midnight" }), "/store/v1/templates/midnight"),
        (methods::STORE_VERSIONS, json!({ "id": "midnight" }), "/store/v1/templates/midnight/versions"),
        (methods::STORE_VERSION, json!({ "id": "midnight", "version": "1.0.0" }), "/store/v1/templates/midnight/versions/1.0.0"),
        (methods::STORE_FILE, json!({ "id": "midnight", "version": "1.0.0", "path": "app.luau" }), "/store/v1/templates/midnight/files/1.0.0/app.luau"),
        (methods::STORE_STATS, json!({}), "/store/v1/stats?kind=template"),
        (methods::STORE_MY_PLUGINS, json!({}), "/store/v1/me/templates"),
        (methods::STORE_VERSION_ACTION, json!({ "id": "midnight", "version": "1.0.0", "action": "yank" }), "/store/v1/templates/midnight/versions/1.0.0/yank"),
        (methods::STORE_REPORT, json!({ "id": "midnight", "reason": "malware" }), "/store/v1/templates/midnight/reports"),
    ];
    let of = |params: &Value, kind: Value| {
        let mut params = params.clone();
        params["kind"] = kind;
        params
    };
    let sent = || store.state.lock().unwrap().requests.len();
    for (method, params, path) in &calls {
        let before = sent();
        let reply = d.call(method, of(params, json!("template"))).await.unwrap_or_else(|e| panic!("{method}: {e}"));
        assert_eq!(reply["kind"], "template", "{method}: {reply}");
        // (A refusal says it too.)
        if [methods::STORE_VERSION_ACTION, methods::STORE_REPORT].contains(method) {
            let mut missing = of(params, json!("template"));
            missing["id"] = json!("not-here");
            let refused = d.call(method, missing).await.unwrap();
            assert_eq!((&refused["ok"], &refused["kind"]), (&json!(false), &json!("template")), "{method}: {refused}");
        }
        let asked: Vec<String> = store.state.lock().unwrap().requests[before..].iter().map(|(_, p, _)| p.clone()).filter(|p| !p.contains("not-here")).collect();
        assert_eq!(asked, [*path], "{method}");
        for odd in [json!("theme"), json!(1), json!("Template"), json!([])] {
            let before = sent();
            assert!(d.call(method, of(params, odd.clone())).await.is_err(), "{method} {odd}");
            assert_eq!(sent(), before, "{method} {odd}: nothing asked");
        }
        // As a plugin's, said or not, it isn't there: a template's id is no
        // plugin's. (Nothing here acts on the plugin of an id.)
        if params.get("id").is_some() {
            for plugin in [params.clone(), of(params, json!("plugin"))] {
                let reply = d.call(method, plugin.clone()).await;
                let refused = reply.is_err() || reply.as_ref().is_ok_and(|r| r["ok"] == false) || *method == methods::STORE_REPORT;
                assert!(refused, "{method} {plugin}: {reply:?}");
            }
        }
    }
    let found = d.call(methods::STORE_SEARCH, json!({ "kind": "template" })).await.unwrap();
    assert!(found["items"].as_array().unwrap().iter().all(|i| i["kind"] == "template" && i["id"] == "midnight"), "{found}");
    assert_eq!(d.call(methods::STORE_STATS, json!({ "kind": "template" })).await.unwrap()["templates"], 1);
    assert_eq!(d.call(methods::STORE_CATEGORIES, json!({ "kind": "template" })).await.unwrap(), json!({ "kind": "template", "categories": [] }));
    assert_eq!(d.call(methods::STORE_MY_PLUGINS, json!({ "kind": "template" })).await.unwrap()["templates"][0]["id"], "midnight");
    let file = d.call(methods::STORE_FILE, json!({ "kind": "template", "id": "midnight", "version": "1.0.0", "path": "app.luau" })).await.unwrap();
    assert_eq!(std::fs::read(file["file"].as_str().unwrap()).unwrap(), b"return 1");
    assert!(d.call(methods::STORE_FILE, json!({ "kind": "template", "id": "midnight", "version": "1.0.0", "path": "zeron-template.json" })).await.is_err());
    // A plugin's answers are as they were: no kind where none was asked of.
    for (method, params) in [(methods::STORE_PLUGIN, json!({ "id": "focus" })), (methods::STORE_SEARCH, json!({})), (methods::STORE_STATS, json!({})), (methods::STORE_VERSIONS, json!({ "id": "focus" }))] {
        assert!(d.call(method, params).await.unwrap().get("kind").is_none(), "{method}");
    }
}

/// What a device chose of a store template is by the store's id, whatever
/// folder it's in, said of each row, and never a plugin's choice.
#[tokio::test]
async fn a_template_s_updates_are_chosen_by_its_store_id() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    store.add_template("ada", &template_package(&key, "midnight", "1.0.0", "return 1"), "listed");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    let templates = d._dir.path().join("templates");
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    store_template(&templates, "midnight", "1.0.0");
    // (Copies of its folder, listed before it and after.)
    for copy in ["another", "other"] {
        std::fs::create_dir_all(templates.join(copy)).unwrap();
        for file in ["zeron-template.json", ".imported.json"] {
            std::fs::copy(templates.join("midnight").join(file), templates.join(copy).join(file)).unwrap();
        }
    }
    std::fs::create_dir_all(templates.join("mine")).unwrap();
    std::fs::write(templates.join("mine/zeron-template.json"), "{}").unwrap();
    let list = || async { d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap() };
    let rows = || async { list().await["templates"].as_array().unwrap().iter().filter(|t| t["id"] == "midnight" || t["id"] == "other").map(|t| (t["prefer"].clone(), t["prerelease"].clone())).collect::<Vec<_>>() };
    assert_eq!((list().await["prefs"].clone(), rows().await), (json!({ "device": "newest" }), vec![(Value::Null, Value::Null); 2]));
    let set = d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": "verified" })).await.unwrap();
    assert_eq!(set, json!({ "prefer": "verified", "device": "newest" }));
    let on = d.call(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": true })).await.unwrap();
    assert_eq!(on, json!({ "id": "midnight", "on": true }));
    assert_eq!(rows().await, vec![(json!("verified"), json!(true)); 2]);
    let page = d.call(methods::STORE_PLUGIN, json!({ "id": "midnight", "kind": "template" })).await.unwrap()["installed"].clone();
    assert_eq!((&page["updates"], &page["folder"], &page["version"], &page["store"], &page["edited"]), (&json!("verified"), &json!("midnight"), &json!("1.0.0"), &json!(true), &json!(true)), "{page}");
    // The device's is over both kinds; a template's own is over it.
    d.call(methods::SET_PLUGIN_UPDATES, json!({ "prefer": "verified" })).await.unwrap();
    assert_eq!(list().await["prefs"]["device"], "verified");
    let set = d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": "newest" })).await.unwrap();
    assert_eq!(set, json!({ "prefer": "newest", "device": "verified" }));
    let set = d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": null })).await.unwrap();
    assert_eq!(set["prefer"], "verified", "as the device does");
    d.call(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": "midnight", "on": false })).await.unwrap();
    assert_eq!(rows().await, vec![(Value::Null, Value::Null); 2]);
    // Not by a folder's name, a plugin's id, what isn't the store's, or no word.
    for id in ["other", "mine", "focus", "", "../midnight"] {
        assert!(d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": id, "prefer": "verified" })).await.is_err(), "{id}");
        assert!(d.call(methods::SET_TEMPLATE_PRERELEASES, json!({ "id": id, "on": true })).await.is_err(), "{id}");
    }
    assert!(d.call(methods::SET_TEMPLATE_UPDATES, json!({ "id": "midnight", "prefer": "oldest" })).await.is_err());
    assert!(d.call(methods::SET_PLUGIN_UPDATES, json!({ "id": "midnight", "prefer": "newest" })).await.is_err(), "a template's isn't a plugin's");
}

/// A package that isn't from the store asks nobody, and needs nothing shown.
#[tokio::test]
async fn a_file_s_template_is_allowed_as_before() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let raw = template_package(&key, "midnight", "1.0.0", "return 1");
    let d = device(&store, Some("bob"));
    let file = d._dir.path().join("midnight.zeron-template");
    std::fs::write(&file, &raw).unwrap();
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let waiting = d.call(methods::IMPORT_TEMPLATE, json!({ "path": file })).await.unwrap();
    let (name, hash) = (&waiting["pending"], &waiting["hash"]);
    let review = || d.call(methods::REVIEW_TEMPLATE, json!({ "pending": name, "hash": hash }));
    let decide = |review: Value| d.call(methods::DECIDE_TEMPLATE, json!({ "pending": name, "allow": true, "hash": hash, "review": review["review"] }));
    // Reviewed as a file's, then said to be the store's: another thing.
    let about = d._dir.path().join("templates/.pending").join(format!("{}.json", name.as_str().unwrap()));
    let was = std::fs::read_to_string(&about).unwrap();
    let good = review().await.unwrap();
    store.offline.store(false, std::sync::atomic::Ordering::SeqCst);
    store.add_template("ada", &raw, "listed");
    let mut forged: Value = serde_json::from_str(&was).unwrap();
    let package = zeron_engine::plugins::template_packages::parse(raw.clone()).unwrap();
    forged["origin"] = json!({ "kind": "store", "version": "1.0.0", "installed_at": 0, "store": { "id": "midnight", "publisher": "ada", "publisher_id": fake_store::publisher_id("ada"), "hash": package.content_hash, "key": package.key, "verified": false } });
    std::fs::write(&about, forged.to_string()).unwrap();
    assert!(decide(good).await.unwrap_err().contains("review"));
    assert!(!d._dir.path().join("templates/midnight").exists());
    std::fs::write(&about, was).unwrap();
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let done = decide(review().await.unwrap()).await.unwrap();
    assert_eq!(done["installed"], "midnight");
    assert!(done["digest"].as_str().is_some_and(|h| h.len() == 64));
}

/// A check that failed repairs nothing, though what it asked about has been
/// deleted since.
#[tokio::test]
async fn a_check_that_failed_repairs_nothing_once_what_it_asked_about_is_gone() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    let file = templates.join(".blocked.json");
    std::fs::write(&file, "nope").unwrap();
    {
        let mut state = store.state.lock().unwrap();
        state.slow_updates = Some(std::time::Duration::from_secs(1));
        state.refuse_template_asks = true;
    }
    let check = d.call(methods::CHECK_PLUGIN_UPDATES, json!({}));
    let deleted = async {
        while !store.state.lock().unwrap().requests.iter().any(|(_, p, _)| p == "/store/v1/updates") {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        std::fs::remove_dir_all(templates.join("midnight")).unwrap();
    };
    let (check, ()) = tokio::join!(check, deleted);
    check.unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"nope");
}

/// A take-down answered beside a template the store's answer can't be read
/// about is kept, however often it's so, and nothing is lifted by it.
#[tokio::test]
async fn a_partial_answer_s_take_down_is_kept() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    store_template(&templates, "odd", "1.0.0");
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.0.0".into()));
    store.state.lock().unwrap().blocked_anyway.push(json!({ "id": "odd", "item_kind": "template" }));
    let kept = || std::fs::read_to_string(templates.join(".blocked.json")).unwrap();
    d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
    assert!(kept().contains("midnight"), "{}", kept());
    let refused = d.call(methods::USE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap_err();
    assert!(refused.contains("taken_down"), "{refused}");
    store.state.lock().unwrap().templates_down.clear();
    d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
    assert!(kept().contains("midnight"), "lifted by a partial answer");
    store.state.lock().unwrap().blocked_anyway.clear();
    d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
    assert!(!kept().contains("midnight"), "lifted by a whole one");
}

/// Only a whole answer lifts anything, or makes a block list that's gone or
/// can't be read good again: a check that failed (offline), that answered for
/// the plugins alone, or about one plugin, leaves it byte for byte as it is.
#[tokio::test]
async fn only_a_whole_answer_lifts_or_repairs() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    store.state.lock().unwrap().templates_down.push(("midnight".into(), "1.0.0".into()));
    d.plugins.check_takedowns().await;
    let file = templates.join(".blocked.json");
    let whole = std::fs::read(&file).unwrap();
    let refused = || async { d.call(methods::USE_TEMPLATE, json!({ "id": "midnight" })).await.is_err_and(|e| e.contains("taken_down")) };
    assert!(refused().await);
    let row = || async { d.call(methods::LIST_TEMPLATES, json!({})).await.unwrap()["templates"].as_array().unwrap().iter().find(|t| t["id"] == "midnight").unwrap().clone() };
    std::fs::create_dir_all(templates.join(".zeron")).unwrap();
    std::fs::write(templates.join(".zeron/zeron-template.json"), r#"{"zeron_version":"0.3.16"}"#).unwrap();
    assert_eq!((&row().await["blocked"]["id"], row().await["blocked"].get("unchecked")), (&json!("midnight"), None));
    let offline = |on: bool| store.offline.store(on, std::sync::atomic::Ordering::SeqCst);
    let refuse = |on: bool| store.state.lock().unwrap().refuse_template_asks = on;
    for (state, bytes) in [("there", Some(whole.clone())), ("gone", None), ("unreadable", Some(b"nope".to_vec())), ("cut short", Some(whole[..whole.len() / 2].to_vec()))] {
        match &bytes {
            Some(bytes) => std::fs::write(&file, bytes).unwrap(),
            None => std::fs::remove_file(&file).unwrap(),
        }
        let same = || std::fs::read(&file).ok() == bytes;
        offline(true);
        let _ = d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await;
        d.plugins.check_takedowns().await;
        offline(false);
        assert!(same(), "{state}: offline");
        refuse(true);
        d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
        d.plugins.check_takedowns().await;
        refuse(false);
        assert!(same(), "{state}: the plugins alone");
        d.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": "focus" })).await.unwrap();
        assert!(same(), "{state}: one plugin");
        assert!(refused().await, "{state}");
        let blocked = row().await["blocked"].clone();
        match state {
            "there" => assert_eq!(blocked["id"], "midnight"),
            _ => assert!(blocked["unchecked"] == true && blocked["reason"].is_string() && blocked.get("id").is_none() && blocked.get("version").is_none() && blocked.get("at").is_none(), "{state}: {blocked}"),
        }
        // (A poll that finds it lost asks about everything again.)
        match state {
            "gone" | "unreadable" => d.plugins.check_takedowns().await,
            _ => drop(d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap()),
        }
        assert!(refused().await, "{state}: the whole answer");
        assert!(std::fs::read_to_string(&file).unwrap().contains("midnight"), "{state}");
    }
}

/// A template whose record can't be read can't be asked about: the place in
/// the feed is taken once, not over again at each look.
#[tokio::test]
async fn a_record_that_cannot_be_read_starts_the_feed_over_once() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("bob"));
    let folder = d._dir.path().join("templates/broken");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("zeron-template.json"), r#"{"id":"broken","name":"Broken"}"#).unwrap();
    std::fs::write(folder.join(".imported.json"), "nope").unwrap();
    for _ in 0..3 {
        d.plugins.check_takedowns().await;
    }
    let starts = store.state.lock().unwrap().requests.iter().filter(|(_, p, _)| p == "/store/v1/takedowns").count();
    assert_eq!(starts, 1);
}

/// Nothing a person does waits on a check that's out: their own check asks
/// about the plugins alone, and using a template asks nobody.
#[tokio::test]
async fn nothing_a_person_does_waits_on_the_templates_check() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    let templates = d._dir.path().join("templates");
    store_template(&templates, "midnight", "1.0.0");
    store.state.lock().unwrap().slow_updates = Some(std::time::Duration::from_secs(3));
    let first = d.call(methods::CHECK_PLUGIN_UPDATES, json!({}));
    let person = async {
        while store.state.lock().unwrap().asks.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        store.state.lock().unwrap().slow_updates = None;
        let started = std::time::Instant::now();
        d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
        d.call(methods::USE_TEMPLATE, json!({ "id": "midnight" })).await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    };
    let (first, ()) = tokio::join!(first, person);
    first.unwrap();
    let asked: Vec<usize> = store.state.lock().unwrap().asks.iter().map(|a| a["templates"].as_array().map_or(0, Vec::len)).collect();
    assert_eq!(asked, [1, 0], "theirs asked about the plugins alone");
}

/// A block this Zeron can't tell the kind of lifts nothing: the plugin it
/// names keeps the block it has.
#[tokio::test]
async fn a_block_of_an_unknown_kind_lifts_no_other() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    store.add("ada", &package_of(&key, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "verified", "");
    let d = device(&store, Some("bob"));
    d.call(methods::INSTALL_PLUGIN, json!({ "store": "focus" })).await.unwrap();
    assert_eq!(d.allow("focus").await["status"], "on");
    store.block("focus", "1.0.0", "Its own.");
    d.plugins.check_takedowns().await;
    let item = d.item("focus").await;
    assert_eq!((item["status"].as_str(), item["blocked"]["reason"].as_str()), (Some("off"), Some("Its own.")), "{item}");
    // Restored in the feed (it's named by nothing), but the full check names
    // it as a kind this Zeron doesn't know.
    store.restore("focus", "1.0.0");
    store.state.lock().unwrap().blocked_anyway.push(json!({ "id": "focus", "version": "1.0.0", "item_kind": "theme" }));
    d.call(methods::CHECK_PLUGIN_UPDATES, json!({})).await.unwrap();
    let item = d.item("focus").await;
    assert_eq!(item["blocked"]["reason"], "Its own.", "{item}");
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

/// The verified version instead is a release; a pre-release only for someone
/// who takes them of it.
#[tokio::test]
async fn the_verified_version_instead_is_a_pre_release_only_for_who_takes_them() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let add = |id: &str, version: &str, state: &str| store.add("ada", &package_of(&key, id, version, &["ui.add"], &format!("return '{version}'"), json!({})), state, "");
    // A verified release, and a newer verified pre-release.
    add("focus", "1.0.0", "verified");
    add("focus", "1.1.0", "listed");
    add("focus", "1.2.0-beta.1", "verified");
    // No verified release.
    add("notes", "1.0.0", "listed");
    add("notes", "1.1.0-beta.1", "verified");
    let d = device(&store, Some("bob"));
    let alternative = async |id: &str, version: &str| d.call(methods::INSTALL_PLUGIN, json!({ "store": id, "version": version })).await.unwrap()["plugin"]["source"]["verified_alternative"]["version"].clone();
    assert_eq!(alternative("focus", "1.1.0").await, "1.0.0");
    assert_eq!(alternative("notes", "1.0.0").await, Value::Null);
    let hash = d.item("notes").await["hash"].clone();
    let refused = d.call(methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": "notes", "hash": hash })).await.unwrap_err();
    assert_eq!(word(&refused), "not_verified", "{refused}");
    for id in ["focus", "notes"] {
        d.call(methods::SET_PLUGIN_PRERELEASES, json!({ "id": id, "on": true })).await.unwrap();
    }
    assert_eq!(alternative("focus", "1.1.0").await, "1.2.0-beta.1");
    assert_eq!(alternative("notes", "1.0.0").await, "1.1.0-beta.1");
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

/// A template's publish not made here is noticed as a plugin's, said to be
/// one, and neither kind's holds up the other's.
#[tokio::test]
async fn a_template_s_publish_not_made_here_is_noticed_too() {
    let store = FakeStore::start().await;
    let (laptop, _) = SigningKey::generate().unwrap();
    store.add_template("ann", &template_package(&laptop, "midnight", "1.0.0", "return 1"), "verified");
    let d = device(&store, Some("ann"));
    let notices = async || {
        d.plugins.check_publishes().await;
        let list = d.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
        list["store_notices"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap().to_owned(), n["kind"].as_str().unwrap().to_owned())).collect::<Vec<_>>()
    };
    assert!(notices().await.is_empty(), "the first look");
    let here = zeron_engine::plugins::signing::load_or_create_key(d._dir.path()).unwrap();
    store.add_template("ann", &template_package(&here, "midnight", "1.1.0", "return 2"), "listed");
    assert!(notices().await.is_empty(), "this device's own");
    store.add_template("ann", &template_package(&laptop, "midnight", "1.2.0", "return 3"), "listed");
    let midnight = ("midnight@1.2.0".to_owned(), "template".to_owned());
    assert_eq!(notices().await, [midnight.clone()]);
    // A store whose templates can't be read: its plugins' are noticed, and
    // theirs when they can be.
    store.state.lock().unwrap().without_templates = true;
    store.add("ann", &package_of(&laptop, "focus", "1.0.0", &["ui.add"], "return 1", json!({})), "listed", "");
    store.add_template("ann", &template_package(&laptop, "midnight", "1.3.0", "return 4"), "listed");
    let focus = ("focus@1.0.0".to_owned(), "plugin".to_owned());
    assert_eq!(notices().await, [midnight.clone(), focus.clone()]);
    store.state.lock().unwrap().without_templates = false;
    assert_eq!(notices().await, [midnight, focus, ("midnight@1.3.0".to_owned(), "template".to_owned())]);
    d.call(methods::ACK_STORE_NOTICES, json!({ "ids": ["midnight@1.2.0", "focus@1.0.0", "midnight@1.3.0"] })).await.unwrap();
    assert!(notices().await.is_empty());
}

/// Your template `id`'s folder on `d`, as you'd be making it.
fn making(d: &Device, id: &str, version: &str, more: Value) -> std::path::PathBuf {
    let folder = d._dir.path().join("templates").join(id);
    std::fs::create_dir_all(&folder).unwrap();
    let mut manifest = json!({ "id": id, "name": "Midnight", "description": "Dark and quiet.", "author": "you", "version": version, "zeron_version": "0.3.16", "license": "MIT" });
    for (key, value) in more.as_object().unwrap() {
        manifest[key] = value.clone();
    }
    std::fs::write(folder.join("zeron-template.json"), manifest.to_string()).unwrap();
    std::fs::write(folder.join("app.luau"), "return 1").unwrap();
    std::fs::write(folder.join("README.md"), "# Midnight\n\nDark.\n").unwrap();
    std::fs::write(folder.join("preview.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    folder
}

/// A template you're making is published as a plugin is: checked, and sent
/// with the ticket its check gave, signed by this device, as it was checked.
#[tokio::test]
async fn your_template_is_published_from_here() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("ann"));
    let folder = making(&d, "midnight", "1.0.0", json!({}));
    let prepare = |id: &str| d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": id }));
    let publish = |of: &Value| d.call(methods::PUBLISH_TEMPLATE, json!({ "id": "midnight", "hash": of["hash"], "ticket": of["ticket"] }));
    let prepared = prepare("midnight").await.unwrap();
    assert_eq!((prepared["ok"].as_bool(), &prepared["kind"], &prepared["id"], &prepared["version"], &prepared["store_checked"]), (Some(true), &json!("template"), &json!("midnight"), &json!("1.0.0"), &json!(true)), "{prepared}");
    assert_eq!((&prepared["package"]["files"], &prepared["warnings"], &prepared["problems"]), (&json!(3), &json!([]), &json!([])), "{prepared}");
    assert!(prepared["listing"].is_object() && prepared.get("note").is_none(), "{prepared}");

    // The person's alone, and what they were shown alone.
    let agent = || zeron_rpc::Caller::Agent { chat: Some("c1".into()) };
    let ask = json!({ "id": "midnight", "hash": prepared["hash"], "ticket": prepared["ticket"] });
    assert!(d.call_as(agent(), methods::PUBLISH_TEMPLATE, ask).await.is_err());
    let theirs = d.call_as(agent(), methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "midnight" })).await;
    assert!(theirs.as_ref().is_ok_and(|p| p.get("ticket").is_none()) || theirs.is_err(), "{theirs:?}");
    assert!(publish(&json!({ "hash": prepared["hash"] })).await.is_err(), "no ticket");
    assert!(publish(&json!({ "hash": "0".repeat(64), "ticket": prepared["ticket"] })).await.is_err(), "another's hash");
    // (A ticket is once, right or wrong, and no plugin's.)
    let prepared = prepare("midnight").await.unwrap();
    assert!(d.call(methods::PUBLISH_PLUGIN, json!({ "id": "midnight", "hash": prepared["hash"], "ticket": prepared["ticket"] })).await.is_err());
    let prepared = prepare("midnight").await.unwrap();
    std::fs::write(folder.join("app.luau"), "return 2").unwrap();
    let changed = publish(&prepared).await.unwrap();
    assert_eq!((&changed["ok"], &changed["kind"], &changed["error"]), (&json!(false), &json!("template"), &json!("changed")), "{changed}");
    assert!(store.state.lock().unwrap().submitted_templates.is_empty());

    let prepared = prepare("midnight").await.unwrap();
    let published = publish(&prepared).await.unwrap();
    assert_eq!((&published["ok"], &published["kind"], &published["version"], &published["status"]), (&json!(true), &json!("template"), &json!("1.0.0"), &json!("listed")), "{published}");
    assert!(published["url"].as_str().unwrap().ends_with("/templates/midnight"), "{published}");
    let (id, raw) = store.state.lock().unwrap().submitted_templates[0].clone();
    let package = zeron_engine::plugins::template_packages::parse_from_store(raw, true).unwrap();
    let key = zeron_engine::plugins::signing::load_key(d._dir.path()).unwrap().public();
    assert_eq!((id.as_str(), package.version().as_str(), Some(&package.content_hash), &package.key), ("midnight", "1.0.0", prepared["hash"].as_str().map(str::to_owned).as_ref(), &Some(key.clone())));
    assert!(store.state.lock().unwrap().submitted.is_empty(), "nothing sent as a plugin");
    // It's noticed nowhere here, and no plugin's made of it.
    d.plugins.check_publishes().await;

    // The same again isn't newer; the store's own refusal is said of it.
    let again = prepare("midnight").await.unwrap();
    assert_eq!((&again["ok"], &again["problems"][0]["rule"], again.get("ticket")), (&json!(false), &json!("version_exists"), None), "{again}");
    making(&d, "midnight", "1.0.1", json!({}));
    let next = prepare("midnight").await.unwrap();
    store.add_template("ann", &template_package(&SigningKey::generate().unwrap().0, "midnight", "1.0.1", "return 9"), "listed");
    let refused = publish(&next).await.unwrap();
    assert_eq!((&refused["ok"], &refused["kind"], &refused["error"]), (&json!(false), &json!("template"), &json!("version_exists")), "{refused}");
    // A store that can't be asked: checked all the same, and said.
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    making(&d, "midnight", "1.0.2", json!({}));
    let unasked = prepare("midnight").await.unwrap();
    assert_eq!((&unasked["ok"], &unasked["store_checked"], unasked["note"].is_string()), (&json!(true), &json!(false), true), "{unasked}");
    let unsent = publish(&unasked).await.unwrap();
    assert_eq!((&unsent["ok"], &unsent["kind"]), (&json!(false), &json!("template")), "{unsent}");
}

/// What a template's folder has and its package doesn't is each said, and
/// stands in no one's way.
#[tokio::test]
async fn what_a_template_s_package_leaves_out_is_said() {
    let store = FakeStore::start().await;
    let d = device(&store, Some("ann"));
    let folder = making(&d, "midnight", "1.0.0", json!({ "signature": "x", "fonts": ["Inter"], "palette": {} }));
    std::fs::create_dir_all(folder.join("lib")).unwrap();
    std::fs::write(folder.join("lib/PROGRA~1.luau"), "1").unwrap();
    std::fs::write(folder.join("lib/no\u{200b}te.luau"), "1").unwrap();
    std::fs::write(folder.join("odd name.luau"), "1").unwrap();
    std::os::unix::fs::symlink(folder.join("app.luau"), folder.join("link.luau")).unwrap();
    std::fs::write(folder.join(".DS_Store"), "1").unwrap();
    let protected = "screens/plugins/page.luau";
    std::fs::create_dir_all(folder.join(protected).parent().unwrap()).unwrap();
    std::fs::write(folder.join(protected), "1").unwrap();
    // (Zeron's own, as it was made with, is counted; changed, it's named.)
    std::fs::create_dir_all(folder.join("account")).unwrap();
    std::fs::write(folder.join("account/page.luau"), "theirs").unwrap();
    std::fs::write(folder.join("account/menu.luau"), "changed").unwrap();
    std::fs::write(folder.join(".base.json"), json!({ "account/page.luau": sha256(b"theirs"), "account/menu.luau": sha256(b"was") }).to_string()).unwrap();
    let prepared = d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "midnight" })).await.unwrap();
    assert_eq!((&prepared["ok"], &prepared["package"]["files"]), (&json!(true), &json!(4)), "{prepared}");
    let left: Vec<&Value> = prepared["warnings"].as_array().unwrap().iter().filter(|w| w["rule"] == "left_out").collect();
    let files: std::collections::BTreeSet<&str> = left.iter().filter(|w| w["field"] == "files").filter_map(|w| w["path"].as_str()).collect();
    let members: Vec<&str> = left.iter().filter(|w| w["field"] == "manifest").map(|w| w["word"].as_str().unwrap()).collect();
    assert_eq!(files, std::collections::BTreeSet::from(["account/menu.luau", "lib/PROGRA~1.luau", "lib/note.luau", "link.luau", protected]));
    assert_eq!(members, ["fonts", "palette"]);
    let counted = |prepared: &Value| prepared["warnings"].as_array().unwrap().iter().filter(|w| w["rule"] == "left_out" && w["field"] == "files" && w.get("path").is_none()).map(|w| w["words"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(counted(&prepared), ["1 of Zeron's own files isn't sent (no template can replace them)."]);
    std::fs::write(folder.join(protected), "theirs").unwrap();
    std::fs::write(folder.join(".base.json"), json!({ "account/page.luau": sha256(b"theirs"), protected: sha256(b"theirs") }).to_string()).unwrap();
    let prepared = d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "midnight" })).await.unwrap();
    assert_eq!(counted(&prepared), ["2 of Zeron's own files aren't sent (no template can replace them)."]);
    std::fs::remove_file(folder.join(".base.json")).unwrap();
    let prepared = d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "midnight" })).await.unwrap();
    assert!(counted(&prepared).is_empty() && prepared["ok"] == true, "{prepared}");
    // The README Zeron wrote isn't one of yours.
    std::fs::write(folder.join("README.md"), zeron_engine::plugins::listing::TEMPLATE_README_STUB).unwrap();
    let prepared = d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "midnight" })).await.unwrap();
    assert!(prepared["warnings"].as_array().unwrap().iter().any(|w| w["rule"] == "left_out" && w["path"] == "README.md"), "{prepared}");
}

/// Only a template you're making here is published from here.
#[tokio::test]
async fn only_your_own_template_is_published_from_here() {
    let store = FakeStore::start().await;
    let (key, _) = SigningKey::generate().unwrap();
    let d = with_midnight(&store, &key).await;
    let refused = async |id: &str| d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": id })).await.unwrap_err();
    for id in ["midnight", "zeron", "nothing-here", "..", "a/b", ".zeron", ""] {
        refused(id).await;
    }
    // Another id in its manifest than its folder's.
    making(&d, "mine", "1.0.0", json!({ "id": "other" }));
    assert!(refused("mine").await.contains("\"mine\""));
    // A copy of one that was taken down.
    let folder = making(&d, "mine", "1.0.0", json!({}));
    assert!(d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "mine" })).await.is_ok());
    let prepared = d.call(methods::PREPARE_TEMPLATE_PUBLISH, json!({ "id": "mine" })).await.unwrap();
    let templates = d._dir.path().join("templates");
    std::fs::write(folder.join(".lineage.json"), json!([{ "id": "bad", "version": "1.0.0", "hash": "a".repeat(64) }]).to_string()).unwrap();
    std::fs::write(templates.join(".blocked.json"), json!({ "format": "zeron-template-blocks:v1", "blocks": [{ "id": "bad", "version": "1.0.0", "hash": "a".repeat(64), "reason": "Bad.", "at": 4_000_000_000u64 }] }).to_string()).unwrap();
    assert!(refused("mine").await.starts_with("taken_down: "));
    let sent = d.call(methods::PUBLISH_TEMPLATE, json!({ "id": "mine", "hash": prepared["hash"], "ticket": prepared["ticket"] })).await.unwrap();
    assert_eq!((&sent["ok"], &sent["kind"], &sent["error"]), (&json!(false), &json!("template"), &json!("taken_down")), "{sent}");
    assert!(store.state.lock().unwrap().submitted_templates.is_empty());
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
