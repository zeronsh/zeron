//! The service around the pure parts: methods end to end on a temp data dir.

use super::keys::Lookup;
use super::keys::tests::FakeDirectory;
use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    data: PathBuf,
    plugins: Plugins,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    let plugins = Plugins::new(&data, "dev-1");
    std::fs::create_dir_all(plugins.plugins_dir()).unwrap();
    Fixture {
        _dir: dir,
        data,
        plugins,
    }
}

async fn call(
    plugins: &Plugins,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Allowing and signing as Zeron's consent window does: with a review.
    let params = super::authority_tests::as_consent_window(plugins, method, params).await;
    match plugins.dispatch(method, params).await {
        Ok(RpcReply::Value(value)) => Ok(value),
        Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
        Err(RpcError::Failed(message)) => Err(message),
        Err(other) => Err(other.to_string()),
    }
}

async fn item(plugins: &Plugins, id: &str) -> serde_json::Value {
    let list = call(plugins, methods::LIST_PLUGINS, serde_json::json!({}))
        .await
        .unwrap();
    list["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no {id} in {list}"))
}

/// A plugin folder, signed by `key` when given.
fn write_plugin(plugins_dir: &Path, id: &str, extra: serde_json::Value, key: Option<&SigningKey>) {
    let dir = plugins_dir.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("init.luau"), "return 1").unwrap();
    let mut manifest =
        serde_json::json!({ "id": id, "name": id, "version": "1.0.0", "entry": "init.luau" });
    for (k, v) in extra.as_object().unwrap() {
        manifest[k] = v.clone();
    }
    std::fs::write(dir.join(MANIFEST_FILE), signing::pretty(&manifest)).unwrap();
    if let Some(key) = key {
        sign_folder(&dir, key).unwrap();
    }
}

#[tokio::test]
async fn plugins_new_plugin_scaffolds_signs_and_is_on() {
    let f = fixture();
    let made = call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "hello", "name": "Hello" }),
    )
    .await
    .unwrap();
    assert_eq!(made["status"], "on", "{made}");
    assert_eq!(made["author"], serde_json::json!({ "kind": "you" }));
    assert_eq!(made["own"], true);
    assert_eq!(made["entry"], "init.luau");
    assert_eq!(made["ask"], serde_json::json!([]));
    let dir = f.plugins.plugins_dir().join("hello");
    let init = std::fs::read_to_string(dir.join("init.luau")).unwrap();
    assert!(
        init.starts_with("local zeron = require(\"./zeron\")(\"hello\")"),
        "{init}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("zeron.luau")).unwrap(),
        scaffold::SDK
    );
    let snapshot = PathBuf::from(made["snapshot"].as_str().unwrap());
    assert!(snapshot.join("init.luau").is_file());
    assert!(snapshot.join("zeron.luau").is_file());
    assert!(snapshot.join(MANIFEST_FILE).is_file());
    // Its words, in English, signed and snapshotted with the rest.
    let english: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("locales/en.json")).unwrap()).unwrap();
    assert_eq!(english["hello"]["toast"], "Hello from Hello");
    assert!(snapshot.join("locales/en.json").is_file());
    assert!(snapshot.starts_with(f.plugins.plugins_dir().join(".loaded/hello")));
    // Signed by this device's key; no decision recorded.
    let key = signing::load_key(&f.data).unwrap();
    assert_eq!(scan::inspect(&dir, true).sig, SigCheck::Valid(key.public()));
    let state = State::load(f.plugins.plugins_dir());
    assert!(state.own.contains("hello"));
    assert!(state.decisions.is_empty());

    // Refusals.
    let again = call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "hello", "name": "Hello" }),
    )
    .await;
    assert_eq!(again.unwrap_err(), "There's already a plugin called hello");
    let reserved = call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "zeron", "name": "Z" }),
    )
    .await;
    assert_eq!(reserved.unwrap_err(), "That name is Zeron's own");
    assert!(
        call(
            &f.plugins,
            methods::NEW_PLUGIN,
            serde_json::json!({ "id": "Bad Id", "name": "B" })
        )
        .await
        .is_err()
    );
    let unknown = call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "x1", "name": "X", "permissions": ["root"] }),
    )
    .await;
    assert!(unknown.unwrap_err().contains("root"));
}

#[tokio::test]
async fn plugins_own_plugin_with_permissions_waits_then_reloads_silently() {
    let f = fixture();
    let made = call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "focus", "name": "Focus", "permissions": ["ui.add"] }),
    )
    .await
    .unwrap();
    assert_eq!(made["status"], "pending");
    assert_eq!(made["ask"], serde_json::json!(["ui.add"]));
    assert!(made.get("snapshot").is_none());

    // A stale hash is refused.
    let stale = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "focus", "allow": true, "hash": "nope" }),
    )
    .await;
    assert_eq!(
        stale.unwrap_err(),
        "It changed while you were looking; review it again"
    );
    let on = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "focus", "allow": true, "hash": made["hash"] }),
    )
    .await
    .unwrap();
    assert_eq!(on["status"], "on");
    assert_eq!(on["allowed"], serde_json::json!(["ui.add"]));

    // An edit: re-signed, still on, a new snapshot.
    let dir = f.plugins.plugins_dir().join("focus");
    std::fs::write(dir.join("init.luau"), "-- edited").unwrap();
    f.plugins.auto_resign("focus");
    let edited = item(&f.plugins, "focus").await;
    assert_eq!(edited["status"], "on");
    assert_ne!(edited["hash"], on["hash"]);
    assert_ne!(edited["snapshot"], on["snapshot"]);
    assert!(
        Path::new(on["snapshot"].as_str().unwrap()).exists(),
        "the previous snapshot stays a while"
    );

    // A new permission asks again, even though it's yours.
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_FILE)).unwrap()).unwrap();
    manifest["permissions"] = serde_json::json!(["ui.add", "network"]);
    std::fs::write(dir.join(MANIFEST_FILE), signing::pretty(&manifest)).unwrap();
    f.plugins.auto_resign("focus");
    let more = item(&f.plugins, "focus").await;
    assert_eq!(more["status"], "pending");
    assert_eq!(more["ask"], serde_json::json!(["network"]));
    assert_eq!(more["author"]["kind"], "you");

    // Off / on.
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_FILE)).unwrap()).unwrap();
    manifest["permissions"] = serde_json::json!(["ui.add"]);
    std::fs::write(dir.join(MANIFEST_FILE), signing::pretty(&manifest)).unwrap();
    f.plugins.auto_resign("focus");
    let off = call(
        &f.plugins,
        methods::SET_PLUGIN_OFF,
        serde_json::json!({ "id": "focus", "off": true }),
    )
    .await
    .unwrap();
    assert_eq!(off["status"], "off");
    assert!(off.get("snapshot").is_none());
    let back = call(
        &f.plugins,
        methods::SET_PLUGIN_OFF,
        serde_json::json!({ "id": "focus", "off": false }),
    )
    .await
    .unwrap();
    assert_eq!(back["status"], "on");
}

#[tokio::test]
async fn plugins_auto_resign_does_not_loop_and_never_signs_others() {
    let f = fixture();
    call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "mine", "name": "Mine" }),
    )
    .await
    .unwrap();
    let dir = f.plugins.plugins_dir().join("mine");
    std::fs::write(dir.join("init.luau"), "-- changed").unwrap();
    f.plugins.auto_resign("mine");
    let written = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
    // The change event for our own write doesn't ask for another re-sign.
    let (mut scan, mut resign) = (false, HashSet::new());
    f.plugins.note(
        Kick::Paths(vec![dir.join(MANIFEST_FILE)]),
        &mut scan,
        &mut resign,
    );
    assert!(resign.is_empty());
    // And re-signing an already signed plugin writes nothing.
    f.plugins.auto_resign("mine");
    assert_eq!(std::fs::read(dir.join(MANIFEST_FILE)).unwrap(), written);
    // A user edit does.
    f.plugins.note(
        Kick::Paths(vec![dir.join("init.luau")]),
        &mut scan,
        &mut resign,
    );
    assert!(resign.contains("mine"));
    // Dotfiles and the engine's own folders are ignored.
    let mut ignored = HashSet::new();
    let mut scan2 = false;
    f.plugins.note(
        Kick::Paths(vec![
            dir.join(".git/index"),
            f.plugins.plugins_dir().join(".loaded/mine/x"),
            f.plugins.plugins_dir().join(".state.json"),
        ]),
        &mut scan2,
        &mut ignored,
    );
    assert!(!scan2 && ignored.is_empty());

    // Not own: never signed.
    write_plugin(
        f.plugins.plugins_dir(),
        "theirs",
        serde_json::json!({}),
        None,
    );
    let mut resign = HashSet::new();
    f.plugins.note(
        Kick::Paths(vec![f.plugins.plugins_dir().join("theirs/init.luau")]),
        &mut scan,
        &mut resign,
    );
    assert!(resign.is_empty());
    f.plugins.auto_resign("theirs");
    assert_eq!(
        scan::inspect(&f.plugins.plugins_dir().join("theirs"), true).sig,
        SigCheck::Unsigned
    );
}

/// Reads are no change: inotify reports a file opened or closed unwritten,
/// and the re-sign above waits for a quiet moment that reads (its own
/// checks, a rescan) would never leave.
#[test]
fn plugins_watcher_ignores_reads() {
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, EventKind, ModifyKind};
    let event = |kind| notify::Event::new(kind).add_path(PathBuf::from("/p/live/init.luau"));
    assert!(!is_change(&event(EventKind::Access(AccessKind::Open(AccessMode::Any)))));
    assert!(!is_change(&event(EventKind::Access(AccessKind::Close(AccessMode::Read)))));
    assert!(!is_change(&event(EventKind::Access(AccessKind::Read))));
    assert!(is_change(&event(EventKind::Access(AccessKind::Close(AccessMode::Write)))));
    assert!(is_change(&event(EventKind::Modify(ModifyKind::Data(DataChange::Any)))));
    assert!(is_change(&event(EventKind::Create(CreateKind::File))));
    assert!(is_change(
        &notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)
    ));
}

#[tokio::test]
async fn plugins_watcher_re_signs_own_plugin_on_save() {
    let f = fixture();
    f.plugins.start();
    call(
        &f.plugins,
        methods::NEW_PLUGIN,
        serde_json::json!({ "id": "live", "name": "Live" }),
    )
    .await
    .unwrap();
    let mut rx = f.plugins.inner.list_tx.subscribe();
    // Let the watch go live.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let dir = f.plugins.plugins_dir().join("live");
    std::fs::write(dir.join("init.luau"), "-- saved").unwrap();
    let key = signing::load_key(&f.data).unwrap().public();
    let signed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let inspected = scan::inspect(&dir, true);
            if inspected.files.get("init.luau") == Some(&signing::sha256_hex(b"-- saved"))
                && inspected.sig == SigCheck::Valid(key.clone())
            {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(200), rx.changed()).await;
        }
    })
    .await;
    assert!(signed.is_ok(), "re-signed after the save");
    // And it settles: no further writes.
    let bytes = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(std::fs::read(dir.join(MANIFEST_FILE)).unwrap(), bytes);
    assert_eq!(item(&f.plugins, "live").await["status"], "on");
    f.plugins.shutdown();
}

#[tokio::test]
async fn plugins_unknown_author_allowed_then_tampered() {
    let f = fixture();
    write_plugin(
        f.plugins.plugins_dir(),
        "loose",
        serde_json::json!({ "permissions": ["ui.add"] }),
        None,
    );
    let found = item(&f.plugins, "loose").await;
    assert_eq!(found["status"], "pending");
    assert_eq!(
        found["author"],
        serde_json::json!({ "kind": "unknown", "reason": "It isn't signed" })
    );
    let on = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "loose", "allow": true, "hash": found["hash"] }),
    )
    .await
    .unwrap();
    assert_eq!(on["status"], "on");
    std::fs::write(
        f.plugins.plugins_dir().join("loose/init.luau"),
        "-- swapped",
    )
    .unwrap();
    let changed = item(&f.plugins, "loose").await;
    assert_eq!(changed["status"], "tampered");
    assert!(changed.get("snapshot").is_none());
    // Declining it.
    let declined = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "loose", "allow": false, "hash": changed["hash"] }),
    )
    .await
    .unwrap();
    assert_eq!(declined["status"], "off");
    assert!(
        call(
            &f.plugins,
            methods::SET_PLUGIN_OFF,
            serde_json::json!({ "id": "loose", "off": false })
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn plugins_verified_author_and_different_author() {
    let f = fixture();
    let fake = Arc::new(FakeDirectory::default());
    let (ada, _) = SigningKey::generate().unwrap();
    let (eve, _) = SigningKey::generate().unwrap();
    fake.keys.lock().unwrap().insert(
        ada.public(),
        Lookup::Found {
            username: Some("ada".into()),
            mine: false,
            revoked: false,
            verified: None,
        },
    );
    fake.keys.lock().unwrap().insert(
        eve.public(),
        Lookup::Found {
            username: Some("eve".into()),
            mine: false,
            revoked: false,
            verified: None,
        },
    );
    f.plugins.set_directory(fake.clone());
    f.plugins.set_account(|| Some("user-1".into()));
    write_plugin(
        f.plugins.plugins_dir(),
        "focus",
        serde_json::json!({ "permissions": ["ui.add"] }),
        Some(&ada),
    );
    let first = item(&f.plugins, "focus").await;
    assert_eq!(first["author"]["reason"], keys::CHECKING);
    // The lookup lands; the next scan knows.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let known = item(&f.plugins, "focus").await;
    assert_eq!(
        known["author"],
        serde_json::json!({ "kind": "verified", "username": "ada" })
    );
    assert_eq!(known["status"], "pending");
    call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "focus", "allow": true, "hash": known["hash"] }),
    )
    .await
    .unwrap();
    // A new version, same author and permissions: silent.
    let mut manifest = serde_json::json!({ "permissions": ["ui.add"], "version": "1.1.0" });
    write_plugin(
        f.plugins.plugins_dir(),
        "focus",
        manifest.clone(),
        Some(&ada),
    );
    assert_eq!(item(&f.plugins, "focus").await["status"], "on");
    // The same id by someone else asks again.
    write_plugin(
        f.plugins.plugins_dir(),
        "focus",
        manifest.clone(),
        Some(&eve),
    );
    let _ = item(&f.plugins, "focus").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let other = item(&f.plugins, "focus").await;
    assert_eq!(other["author"]["username"], "eve");
    assert_eq!(other["status"], "pending");
    assert_eq!(other["ask"], serde_json::json!(["ui.add"]));
    // Unreachable and not cached.
    let (zed, _) = SigningKey::generate().unwrap();
    fake.unreachable.store(true, Ordering::SeqCst);
    manifest["version"] = serde_json::json!("2");
    write_plugin(f.plugins.plugins_dir(), "other", manifest, Some(&zed));
    let _ = item(&f.plugins, "other").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        item(&f.plugins, "other").await["author"]["reason"],
        keys::COULD_NOT_CHECK
    );
    // Its own key is you, with no lookup, even signed out.
    f.plugins.set_account(|| None);
    let own = signing::load_or_create_key(&f.data).unwrap();
    write_plugin(
        f.plugins.plugins_dir(),
        "mykey",
        serde_json::json!({}),
        Some(&own),
    );
    let mine = item(&f.plugins, "mykey").await;
    assert_eq!(mine["author"]["kind"], "you");
    assert_eq!(mine["own"], false);
    assert_eq!(mine["status"], "on");
}

#[tokio::test]
async fn plugins_list_is_sorted_and_reports_broken() {
    let f = fixture();
    write_plugin(
        f.plugins.plugins_dir(),
        "zz",
        serde_json::json!({ "name": "Alpha" }),
        None,
    );
    write_plugin(
        f.plugins.plugins_dir(),
        "aa",
        serde_json::json!({ "name": "beta" }),
        None,
    );
    write_plugin(
        f.plugins.plugins_dir(),
        "bad",
        serde_json::json!({ "name": "Gamma", "mcp": { "command": "./x" } }),
        None,
    );
    std::fs::create_dir_all(f.plugins.plugins_dir().join("app")).unwrap();
    #[cfg(unix)]
    {
        write_plugin(
            f.plugins.plugins_dir(),
            "linky",
            serde_json::json!({ "name": "Delta" }),
            None,
        );
        std::os::unix::fs::symlink("/etc/hosts", f.plugins.plugins_dir().join("linky/hosts"))
            .unwrap();
    }
    let list = call(&f.plugins, methods::LIST_PLUGINS, serde_json::json!({}))
        .await
        .unwrap();
    let names: Vec<&str> = list["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    #[cfg(unix)]
    assert_eq!(names, vec!["Alpha", "app", "beta", "Delta", "Gamma"]);
    let by_id = |id: &str| {
        list["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by_id("bad")["status"], "broken");
    assert!(by_id("bad")["error"].as_str().unwrap().contains("native"));
    assert_eq!(by_id("app")["error"], "That name is Zeron's own");
    #[cfg(unix)]
    assert_eq!(by_id("linky")["error"], files::LINK_ERROR);
    assert!(
        list["dirs"]["plugins"]
            .as_str()
            .unwrap()
            .ends_with("plugins")
    );
    assert!(
        list["dirs"]["templates"]
            .as_str()
            .unwrap()
            .ends_with("templates")
    );

    // Remove.
    call(
        &f.plugins,
        methods::REMOVE_PLUGIN,
        serde_json::json!({ "id": "aa" }),
    )
    .await
    .unwrap();
    assert!(!f.plugins.plugins_dir().join("aa").exists());
    let removed: Vec<_> = std::fs::read_dir(f.plugins.plugins_dir().join(REMOVED_DIR))
        .unwrap()
        .flatten()
        .collect();
    assert!(removed[0].file_name().to_string_lossy().starts_with("aa-"));
    assert!(
        call(
            &f.plugins,
            methods::REMOVE_PLUGIN,
            serde_json::json!({ "id": "../x" })
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn plugins_verify_any_folder_and_sign_only_in_the_plugins_folder() {
    let f = fixture();
    let outside = tempfile::tempdir().unwrap();
    write_plugin(outside.path(), "ext", serde_json::json!({}), None);
    let folder = outside.path().join("ext");
    let path = folder.display().to_string();
    let unsigned = call(
        &f.plugins,
        methods::VERIFY_PLUGIN,
        serde_json::json!({ "path": path }),
    )
    .await
    .unwrap();
    assert_eq!(unsigned["status"], "unsigned");
    // This device's key signs nothing outside the plugins folder (R4).
    assert!(call(&f.plugins, methods::SIGN_PLUGIN, serde_json::json!({ "path": path })).await.is_err());
    let key = signing::load_or_create_key(&f.data).unwrap();
    sign_folder(&folder, &key).unwrap();
    let signed = call(&f.plugins, methods::VERIFY_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    assert_eq!(signed["status"], "ok");
    assert_eq!(signed["author"]["kind"], "you");
    std::fs::write(folder.join("extra.luau"), "x").unwrap();
    let tampered = call(
        &f.plugins,
        methods::VERIFY_PLUGIN,
        serde_json::json!({ "path": path }),
    )
    .await
    .unwrap();
    assert_eq!(tampered["status"], "tampered");
    assert_eq!(tampered["author"]["reason"], scan::FILES_DIFFER);
    // Not added to own: it isn't under the plugins folder.
    assert!(State::load(f.plugins.plugins_dir()).own.is_empty());

    // Signing one in the plugins folder by id makes it own.
    write_plugin(
        f.plugins.plugins_dir(),
        "adopt",
        serde_json::json!({ "permissions": ["tools"] }),
        None,
    );
    let adopted = call(
        &f.plugins,
        methods::SIGN_PLUGIN,
        serde_json::json!({ "id": "adopt" }),
    )
    .await
    .unwrap();
    assert_eq!(adopted["own"], true);
    assert_eq!(adopted["status"], "pending", "no automatic decision");
    assert_eq!(adopted["ask"], serde_json::json!(["tools"]));

    let key = call(
        &f.plugins,
        methods::PLUGIN_SIGNING_KEY,
        serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(key["key"], signing::load_key(&f.data).unwrap().public());
    assert_eq!(key["registered"], false);
}

/// A plugin, `on`, with `extra` permissions allowed.
async fn allowed_plugin(f: &Fixture, id: &str, extra: serde_json::Value) -> serde_json::Value {
    write_plugin(f.plugins.plugins_dir(), id, extra, None);
    let found = item(&f.plugins, id).await;
    call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": id, "allow": true, "hash": found["hash"] }),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn plugins_tools_through_the_broker_over_rpc() {
    struct Service(Plugins);
    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Service {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            self.0.dispatch(method, params).await
        }
    }
    let f = fixture();
    let on = allowed_plugin(
        &f,
        "word-count",
        serde_json::json!({
            "name": "Word count",
            "permissions": ["tools"],
            "tools": [{ "name": "count_words", "description": "Count words", "input": { "type": "object" } }],
        }),
    )
    .await;
    assert_eq!(on["status"], "on");
    let client = Arc::new(zeron_rpc::memory_client(Arc::new(Service(
        f.plugins.clone(),
    ))));
    let tools = client
        .call(methods::LIST_PLUGIN_TOOLS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(tools["device"]["id"], "dev-1");
    assert_eq!(tools["tools"][0]["mcp_name"], "word_count__count_words");
    assert_eq!(tools["tools"][0]["plugin_name"], "Word count");
    assert_eq!(tools["tools"][0]["open"], false);

    let mut watching = client
        .subscribe(methods::WATCH_PLUGIN_TOOLS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(watching.recv().await.unwrap()["tools"][0]["open"], false);

    let closed = client
        .call(
            methods::CALL_PLUGIN_TOOL,
            serde_json::json!({ "mcp_name": "word_count__count_words", "args": {} }),
        )
        .await
        .unwrap_err();
    assert!(closed.to_string().starts_with("Open Zeron on "), "{closed}");
    assert!(closed.to_string().ends_with(" to use count_words"));
    let unknown = client
        .call(
            methods::CALL_PLUGIN_TOOL,
            serde_json::json!({ "mcp_name": "nope__x" }),
        )
        .await
        .unwrap_err();
    assert_eq!(unknown.to_string(), "There's no tool nope__x");

    let mut serving = client
        .subscribe(methods::SERVE_PLUGIN_TOOLS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(watching.recv().await.unwrap()["tools"][0]["open"], true);
    let app = client.clone();
    let answering = tokio::spawn(async move {
        let call = serving.recv().await.unwrap();
        assert_eq!(call["plugin"], "word-count");
        assert_eq!(call["tool"], "count_words");
        assert_eq!(call["chatId"], "chat-9");
        assert_eq!(call["args"]["text"], "one two");
        app.call(
            methods::PLUGIN_TOOL_RESULT,
            serde_json::json!({ "callId": call["callId"], "ok": true, "value": { "words": 2 } }),
        )
        .await
        .unwrap();
        let call = serving.recv().await.unwrap();
        app.call(
            methods::PLUGIN_TOOL_RESULT,
            serde_json::json!({ "callId": call["callId"], "ok": false, "error": "no text" }),
        )
        .await
        .unwrap();
        serving
    });
    let reply = client
        .call(
            methods::CALL_PLUGIN_TOOL,
            serde_json::json!({ "plugin": "word-count", "tool": "count_words", "args": { "text": "one two" }, "chatId": "chat-9" }),
        )
        .await
        .unwrap();
    assert_eq!(
        reply,
        serde_json::json!({ "ok": true, "value": { "words": 2 } })
    );
    let failed = client
        .call(
            methods::CALL_PLUGIN_TOOL,
            serde_json::json!({ "mcp_name": "word_count__count_words" }),
        )
        .await
        .unwrap_err();
    assert_eq!(failed.to_string(), "no text");
    drop(answering.await.unwrap());
}

#[tokio::test]
async fn plugins_deliver_checks_on_the_receiving_side() {
    let f = fixture();
    let deliver = |plugin: &str, key: Option<String>| serde_json::json!({ "plugin": plugin, "from": "dev-2", "author_key": key, "data": { "n": 1 } });
    // Not installed.
    let r = call(&f.plugins, methods::PLUGIN_DELIVER, deliver("ghost", None))
        .await
        .unwrap();
    assert_eq!(r["outcome"], "not_installed");
    // Installed, not allowed devices.
    allowed_plugin(&f, "chat", serde_json::json!({ "permissions": ["ui.add"] })).await;
    let r = call(&f.plugins, methods::PLUGIN_DELIVER, deliver("chat", None))
        .await
        .unwrap();
    assert_eq!(r["outcome"], "not_allowed");
    // Allowed, nobody listening.
    allowed_plugin(
        &f,
        "sync",
        serde_json::json!({ "permissions": ["devices"] }),
    )
    .await;
    let r = call(&f.plugins, methods::PLUGIN_DELIVER, deliver("sync", None))
        .await
        .unwrap();
    assert_eq!(r["outcome"], "not_open");
    // Listening.
    let mut channel = match f
        .plugins
        .dispatch(methods::WATCH_PLUGIN_CHANNEL, serde_json::json!({}))
        .await
    {
        Ok(RpcReply::Stream(stream)) => stream,
        _ => panic!("stream"),
    };
    let r = call(&f.plugins, methods::PLUGIN_DELIVER, deliver("sync", None))
        .await
        .unwrap();
    assert_eq!(r["outcome"], "delivered");
    let message = channel.next().await.unwrap();
    assert_eq!(message["plugin"], "sync");
    assert_eq!(
        message["from"],
        serde_json::json!({ "id": "dev-2", "name": "dev-2" })
    );
    assert_eq!(message["data"], serde_json::json!({ "n": 1 }));
    assert!(message["at"].as_i64().unwrap() > 0);

    // Signed on both sides: the keys' accounts must match.
    let fake = Arc::new(FakeDirectory::default());
    let (ada1, _) = SigningKey::generate().unwrap();
    let (ada2, _) = SigningKey::generate().unwrap();
    let (eve, _) = SigningKey::generate().unwrap();
    for (key, user) in [(&ada1, "ada"), (&ada2, "ada"), (&eve, "eve")] {
        fake.keys.lock().unwrap().insert(
            key.public(),
            Lookup::Found {
                username: Some(user.into()),
                mine: false,
                revoked: false,
                verified: None,
            },
        );
    }
    f.plugins.set_directory(fake);
    f.plugins.set_account(|| Some("u".into()));
    write_plugin(
        f.plugins.plugins_dir(),
        "signed",
        serde_json::json!({ "permissions": ["devices"] }),
        Some(&ada1),
    );
    let _ = item(&f.plugins, "signed").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let found = item(&f.plugins, "signed").await;
    call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "signed", "allow": true, "hash": found["hash"] }),
    )
    .await
    .unwrap();
    let r = call(
        &f.plugins,
        methods::PLUGIN_DELIVER,
        deliver("signed", Some(eve.public())),
    )
    .await
    .unwrap();
    assert_eq!(r["outcome"], "not_allowed");
    let r = call(
        &f.plugins,
        methods::PLUGIN_DELIVER,
        deliver("signed", Some(ada2.public())),
    )
    .await
    .unwrap();
    assert_eq!(r["outcome"], "delivered");
    let r = call(
        &f.plugins,
        methods::PLUGIN_DELIVER,
        deliver("signed", Some(ada1.public())),
    )
    .await
    .unwrap();
    assert_eq!(r["outcome"], "delivered");
}

#[tokio::test]
async fn plugins_send_checks_and_targets() {
    let f = fixture();
    allowed_plugin(
        &f,
        "quiet",
        serde_json::json!({ "permissions": ["ui.add"] }),
    )
    .await;
    let refused = call(
        &f.plugins,
        methods::PLUGIN_SEND,
        serde_json::json!({ "plugin": "quiet", "to": "*", "data": 1 }),
    )
    .await;
    assert_eq!(
        refused.unwrap_err(),
        "quiet isn't allowed to reach your other devices"
    );
    allowed_plugin(
        &f,
        "loud",
        serde_json::json!({ "permissions": ["devices"] }),
    )
    .await;
    let big = "x".repeat(MAX_SEND_BYTES + 1);
    assert!(
        call(
            &f.plugins,
            methods::PLUGIN_SEND,
            serde_json::json!({ "plugin": "loud", "to": "*", "data": big })
        )
        .await
        .is_err()
    );
    // No device links: offline. To this device: delivered locally.
    let _channel = f
        .plugins
        .dispatch(methods::WATCH_PLUGIN_CHANNEL, serde_json::json!({}))
        .await
        .unwrap();
    let sent = call(
        &f.plugins,
        methods::PLUGIN_SEND,
        serde_json::json!({ "plugin": "loud", "to": "dev-2", "data": { "hi": true } }),
    )
    .await
    .unwrap();
    assert_eq!(
        sent,
        serde_json::json!({ "results": { "dev-2": "offline" } })
    );
    let sent = call(
        &f.plugins,
        methods::PLUGIN_SEND,
        serde_json::json!({ "plugin": "loud", "to": "dev-1", "data": { "hi": true } }),
    )
    .await
    .unwrap();
    assert_eq!(
        sent,
        serde_json::json!({ "results": { "dev-1": "delivered" } })
    );
    // "*" without a device registry: nobody.
    let sent = call(
        &f.plugins,
        methods::PLUGIN_SEND,
        serde_json::json!({ "plugin": "loud", "to": "*", "data": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(sent, serde_json::json!({ "results": {} }));
}

#[tokio::test]
async fn plugins_native_mcp_servers_run_from_the_snapshot() {
    let f = fixture();
    let manifest = serde_json::json!({
        "permissions": ["native"],
        "mcp": { "command": "./bin/server", "args": ["--stdio"], "env": { "A": "1" } },
    });
    write_plugin(f.plugins.plugins_dir(), "native", manifest, None);
    std::fs::create_dir_all(f.plugins.plugins_dir().join("native/bin")).unwrap();
    std::fs::write(
        f.plugins.plugins_dir().join("native/bin/server"),
        "#!/bin/sh\n",
    )
    .unwrap();
    let pending = item(&f.plugins, "native").await;
    assert_eq!(pending["mcp"], true);
    assert!(
        f.plugins.native_mcp_servers().is_empty(),
        "never without a decision"
    );
    let on = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        serde_json::json!({ "id": "native", "allow": true, "hash": pending["hash"] }),
    )
    .await
    .unwrap();
    let servers = f.plugins.native_mcp_servers();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "plugin-native");
    let snapshot = on["snapshot"].as_str().unwrap();
    assert_eq!(
        servers[0].command,
        Path::new(snapshot).join("bin").join("server").display().to_string()
    );
    assert!(Path::new(&servers[0].command).is_file());
    assert_eq!(servers[0].args, vec!["--stdio".to_string()]);
    assert_eq!(servers[0].env.get("A").map(String::as_str), Some("1"));
}

#[tokio::test]
async fn plugins_watch_plugins_emits_at_once_then_on_change() {
    let f = fixture();
    let mut stream = match f
        .plugins
        .dispatch(methods::WATCH_PLUGINS, serde_json::json!({}))
        .await
    {
        Ok(RpcReply::Stream(stream)) => stream,
        _ => panic!("stream"),
    };
    let first = stream.next().await.unwrap();
    assert_eq!(first["plugins"], serde_json::json!([]));
    write_plugin(f.plugins.plugins_dir(), "new", serde_json::json!({}), None);
    f.plugins.rescan().await;
    let second = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second["plugins"][0]["id"], "new");
    // An unchanged rescan pushes nothing.
    f.plugins.rescan().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn plugins_zeron_s_own_are_allowed_what_they_ask_turn_off_and_never_go() {
    let f = fixture();
    let shipped = f.data.join("app-plugins");
    std::fs::create_dir_all(&shipped).unwrap();
    write_plugin(&shipped, "github", serde_json::json!({ "name": "GitHub", "permissions": ["ui.add", "github"] }), None);
    // One of yours by the same name gives way.
    write_plugin(f.plugins.plugins_dir(), "github", serde_json::json!({ "name": "Mine" }), None);
    call(&f.plugins, methods::SET_BUILTIN_PLUGINS, serde_json::json!({ "dir": shipped }))
        .await
        .unwrap();
    let list = call(&f.plugins, methods::LIST_PLUGINS, serde_json::json!({})).await.unwrap();
    let all = list["plugins"].as_array().unwrap();
    let theirs = all.iter().find(|p| p["id"] == "github" && p["builtin"] == true).expect("Zeron's");
    assert_eq!(theirs["author"]["kind"], "zeron");
    assert_eq!(theirs["status"], "on");
    assert_eq!(theirs["allowed"], serde_json::json!(["github", "ui.add"]));
    assert!(theirs["snapshot"].is_string(), "on means a snapshot to load: {theirs}");
    let mine = all.iter().find(|p| p["id"] == "github" && p["builtin"] != true).expect("yours");
    assert_eq!(mine["status"], "broken");
    // Off, and on again; never removed.
    call(&f.plugins, methods::SET_PLUGIN_OFF, serde_json::json!({ "id": "github", "off": true })).await.unwrap();
    let list = call(&f.plugins, methods::LIST_PLUGINS, serde_json::json!({})).await.unwrap();
    let off = list["plugins"].as_array().unwrap().iter().find(|p| p["builtin"] == true).unwrap().clone();
    assert_eq!(off["status"], "off");
    let removed = call(&f.plugins, methods::REMOVE_PLUGIN, serde_json::json!({ "id": "github" })).await;
    assert!(removed.unwrap_err().contains("turned off, not removed"));
    // A new version asking more is allowed it as it ships.
    write_plugin(&shipped, "github", serde_json::json!({ "name": "GitHub", "permissions": ["ui.add", "github", "panel.tabs"] }), None);
    call(&f.plugins, methods::SET_PLUGIN_OFF, serde_json::json!({ "id": "github", "off": false })).await.unwrap();
    let list = call(&f.plugins, methods::LIST_PLUGINS, serde_json::json!({})).await.unwrap();
    let on = list["plugins"].as_array().unwrap().iter().find(|p| p["builtin"] == true).unwrap().clone();
    assert_eq!((on["status"].as_str(), on["allowed"].as_array().map(Vec::len)), (Some("on"), Some(3)));
}

#[tokio::test]
async fn plugins_zeron_s_own_load_from_the_app_s_folder_with_none_of_yours_beside_it() {
    // (The test above has a folder of yours by the same name, which hid that
    // the snapshot was copied from `<data>/plugins/<id>`: Zeron's own never
    // had one, so it never loaded.)
    let f = fixture();
    let shipped = f.data.join("app-plugins");
    std::fs::create_dir_all(&shipped).unwrap();
    write_plugin(&shipped, "github", serde_json::json!({ "name": "GitHub", "permissions": ["ui.add", "github"] }), None);
    call(&f.plugins, methods::SET_BUILTIN_PLUGINS, serde_json::json!({ "dir": shipped }))
        .await
        .unwrap();
    let list = call(&f.plugins, methods::LIST_PLUGINS, serde_json::json!({})).await.unwrap();
    let github = list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == "github").unwrap().clone();
    assert_eq!(github["status"], "on");
    let snapshot = PathBuf::from(github["snapshot"].as_str().expect("on means a snapshot to load"));
    assert!(snapshot.join("init.luau").is_file(), "its files copied from the app's folder: {snapshot:?}");
}

// ── Third-party lifecycle: install, update, uninstall ──────────────────────

use super::install::tests::signed_package;

/// `key` known to the directory as `username`'s (someone else's).
fn someone(f: &Fixture, fake: &Arc<FakeDirectory>, username: &str) -> SigningKey {
    let (key, _) = SigningKey::generate().unwrap();
    fake.keys.lock().unwrap().insert(
        key.public(),
        Lookup::Found { username: Some(username.into()), mine: false, revoked: false, verified: None },
    );
    f.plugins.set_directory(fake.clone());
    f.plugins.set_account(|| Some("user-1".into()));
    key
}

fn package_file(f: &Fixture, name: &str, raw: &[u8]) -> PathBuf {
    let path = f.data.join("downloads").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, raw).unwrap();
    path
}

#[tokio::test]
async fn plugins_install_from_a_package_waits_for_consent_unless_yours() {
    let f = fixture();
    let fake = Arc::new(FakeDirectory::default());
    let ada = someone(&f, &fake, "ada");
    let path = package_file(&f, "focus.zeron-plugin", &signed_package(&ada, "focus", "1.0.0", &["ui.add"], "return 1"));
    let installed = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    assert_eq!(installed["update"], false);
    assert_eq!(installed["waiting"], true, "{installed}");
    let plugin = &installed["plugin"];
    assert_eq!(plugin["status"], "pending");
    assert_eq!(plugin["author"], serde_json::json!({ "kind": "verified", "username": "ada" }));
    assert_eq!(plugin["ask"], serde_json::json!(["ui.add"]));
    assert_eq!(plugin["source"]["kind"], "file");
    assert!(plugin["source"].get("path").is_none(), "never a local path in the list");
    // The consent window's decision turns it on.
    call(&f.plugins, methods::DECIDE_PLUGIN, serde_json::json!({ "id": "focus", "allow": true, "hash": plugin["hash"] }))
        .await
        .unwrap();
    assert_eq!(item(&f.plugins, "focus").await["status"], "on");

    // Signed by this device's key: on at once, no asking.
    let own = signing::load_or_create_key(&f.data).unwrap();
    let path = package_file(&f, "mine.zeron-plugin", &signed_package(&own, "mine", "1.0.0", &["ui.add", "links.open"], "return 1"));
    let mine = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    assert_eq!((mine["plugin"]["status"].as_str(), mine["waiting"].as_bool()), (Some("on"), Some(false)), "{mine}");
    assert_eq!(mine["plugin"]["allowed"], serde_json::json!(["links.open", "ui.add"]));
    assert_eq!(mine["plugin"]["own"], false, "installed, not one you're editing here");

    // Tampered (a file changed after signing): refused, nothing written.
    let mut value: serde_json::Value =
        serde_json::from_slice(&signed_package(&ada, "evil", "1.0.0", &[], "return 1")).unwrap();
    value["files"]["init.luau"] = serde_json::json!(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"return 2"));
    let path = package_file(&f, "evil.zeron-plugin", &serde_json::to_vec(&value).unwrap());
    let refused = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap_err();
    assert_eq!(refused, install::WORDS.tampered);
    assert!(!f.plugins.plugins_dir().join("evil").exists());
    // Not a package at all.
    let path = package_file(&f, "x.zeron-plugin", b"{}");
    assert_eq!(call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap_err(), install::WORDS.not_a_package);
}

#[tokio::test]
async fn plugins_updates_keep_consent_ask_for_new_permissions_and_refuse_tampering() {
    let f = fixture();
    let fake = Arc::new(FakeDirectory::default());
    let ada = someone(&f, &fake, "ada");
    let path = package_file(&f, "focus.zeron-plugin", &signed_package(&ada, "focus", "1.0.0", &["ui.add"], "return 1"));
    let installed = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    call(&f.plugins, methods::DECIDE_PLUGIN, serde_json::json!({ "id": "focus", "allow": true, "hash": installed["plugin"]["hash"] }))
        .await
        .unwrap();
    // Nothing new at the source.
    let checked = call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({})).await.unwrap();
    assert_eq!(checked["updates"], serde_json::json!([]), "{checked}");
    assert!(call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap_err().contains("up to date"));

    // A tampered new version at the source: refused, the installed one kept.
    let mut value: serde_json::Value =
        serde_json::from_slice(&signed_package(&ada, "focus", "1.1.0", &["ui.add"], "return 2")).unwrap();
    value["files"]["init.luau"] = serde_json::json!(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"steal()"));
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let checked = call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({})).await.unwrap();
    assert_eq!(checked["errors"][0]["error"], install::WORDS.tampered, "{checked}");
    assert_eq!(call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap_err(), install::WORDS.tampered);
    let kept = item(&f.plugins, "focus").await;
    assert_eq!((kept["version"].as_str(), kept["status"].as_str()), (Some("1.0.0"), Some("on")));

    // Signed by someone else: found, but refused.
    let eve = someone(&f, &fake, "eve");
    std::fs::write(&path, signed_package(&eve, "focus", "6.6.6", &[], "return 6")).unwrap();
    let checked = call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({})).await.unwrap();
    assert!(checked["updates"][0]["refused"].as_str().unwrap().contains("someone else"), "{checked}");
    assert!(call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap_err().contains("someone else"));
    assert_eq!(item(&f.plugins, "focus").await["version"], "1.0.0");

    // Same key, same permissions: updated, still on.
    std::fs::write(&path, signed_package(&ada, "focus", "1.1.0", &["ui.add"], "return 2")).unwrap();
    let checked = call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({})).await.unwrap();
    assert_eq!(checked["updates"][0]["version"], "1.1.0");
    let listed = item(&f.plugins, "focus").await;
    assert_eq!(listed["update"], serde_json::json!({ "version": "1.1.0", "added": [], "removed": [] }));
    let updated = call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap();
    assert_eq!((updated["update"].as_bool(), updated["from"].as_str(), updated["waiting"].as_bool()), (Some(true), Some("1.0.0"), Some(false)));
    let now = item(&f.plugins, "focus").await;
    assert_eq!((now["version"].as_str(), now["status"].as_str()), (Some("1.1.0"), Some("on")));
    assert!(now.get("update").is_none(), "the update installed is forgotten: {now}");

    // A new permission: installed, but it waits for you, asking only that.
    std::fs::write(&path, signed_package(&ada, "focus", "2.0.0", &["ui.add", "github"], "return 3")).unwrap();
    let updated = call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["added"], serde_json::json!(["github"]));
    assert_eq!(updated["waiting"], true);
    let waiting = item(&f.plugins, "focus").await;
    assert_eq!(waiting["status"], "pending");
    assert_eq!((waiting["allowed"].clone(), waiting["ask"].clone()), (serde_json::json!(["ui.add"]), serde_json::json!(["github"])));
    call(&f.plugins, methods::DECIDE_PLUGIN, serde_json::json!({ "id": "focus", "allow": true, "hash": waiting["hash"] }))
        .await
        .unwrap();
    assert_eq!(item(&f.plugins, "focus").await["status"], "on");
}

#[tokio::test]
async fn plugins_an_unknown_authors_update_keeps_consent_by_key() {
    let f = fixture();
    // Signed out: no author can be checked.
    let (zed, _) = SigningKey::generate().unwrap();
    let path = package_file(&f, "z.zeron-plugin", &signed_package(&zed, "zed", "1", &["ui.add"], "return 1"));
    let installed = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    assert_eq!(installed["plugin"]["author"]["kind"], "unknown");
    call(&f.plugins, methods::DECIDE_PLUGIN, serde_json::json!({ "id": "zed", "allow": true, "hash": installed["plugin"]["hash"] }))
        .await
        .unwrap();
    // Updated, signed by the same key: nobody knows whose key that is, and
    // the decision was for the content, so it waits for you again (R3).
    std::fs::write(&path, signed_package(&zed, "zed", "2", &["ui.add"], "return 2")).unwrap();
    let updated = call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "zed" })).await.unwrap();
    assert_eq!(updated["plugin"]["status"], "pending", "same key, same asks: {updated}");
    call(&f.plugins, methods::DECIDE_PLUGIN, serde_json::json!({ "id": "zed", "allow": true, "hash": updated["plugin"]["hash"] }))
        .await
        .unwrap();
    std::fs::write(&path, signed_package(&zed, "zed", "3", &["ui.add", "notifications"], "return 3")).unwrap();
    let updated = call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "zed" })).await.unwrap();
    assert_eq!(updated["plugin"]["status"], "pending");
    assert_eq!(updated["plugin"]["ask"], serde_json::json!(["notifications"]));
    // Changed by hand after that: tampered, as before.
    std::fs::write(f.plugins.plugins_dir().join("zed/init.luau"), "changed()").unwrap();
    assert_eq!(item(&f.plugins, "zed").await["status"], "tampered");
}

#[tokio::test]
async fn plugins_install_from_git_and_update_from_its_branch() {
    let f = fixture();
    let fake = Arc::new(FakeDirectory::default());
    let ada = someone(&f, &fake, "ada");
    // A repository holding the plugin in a folder, as an author publishes it.
    let repo = f.data.join("repo");
    let folder = repo.join("plugins/focus");
    let publish = |version: &str, permissions: &[&str], body: &str| {
        let package = install::parse(signed_package(&ada, "focus", version, permissions, body)).unwrap();
        std::fs::create_dir_all(folder.join("ui")).unwrap();
        for (rel, bytes) in &package.files {
            std::fs::write(folder.join(rel), bytes).unwrap();
        }
        std::fs::write(folder.join(MANIFEST_FILE), signing::pretty(&package.manifest.raw)).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        };
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", version]);
    };
    std::fs::create_dir_all(&repo).unwrap();
    assert!(std::process::Command::new("git").args(["init", "-q", "-b", "main"]).current_dir(&repo).status().unwrap().success());
    publish("1.0.0", &["ui.add"], "return 1");
    let url = format!("file://{}", repo.display());
    let installed = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "url": url, "folder": "plugins/focus" }))
        .await
        .unwrap();
    assert_eq!(installed["plugin"]["status"], "pending", "{installed}");
    assert_eq!(installed["plugin"]["source"], serde_json::json!({ "kind": "git", "url": url }));
    let source = install::Source::load(&f.plugins.plugins_dir().join("focus")).unwrap();
    assert_eq!(source.path.as_deref(), Some("plugins/focus"));
    assert!(source.commit.is_some());
    // Nothing left of the clone.
    let incoming = f.plugins.plugins_dir().join(install::INCOMING_DIR);
    assert_eq!(std::fs::read_dir(&incoming).unwrap().count(), 0);
    // The branch unchanged: no update, without cloning.
    let checked = call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({})).await.unwrap();
    assert_eq!(checked, serde_json::json!({ "updates": [], "errors": [] }));
    // A new commit with a new permission.
    publish("1.1.0", &["ui.add", "links.open"], "return 2");
    let checked = call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({ "id": "focus" })).await.unwrap();
    assert_eq!(checked["updates"][0]["added"], serde_json::json!(["links.open"]), "{checked}");
    let updated = call(&f.plugins, methods::UPDATE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap();
    assert_eq!(updated["plugin"]["version"], "1.1.0");
    let moved = install::Source::load(&f.plugins.plugins_dir().join("focus")).unwrap();
    assert_ne!(moved.commit, source.commit, "the source follows the commit installed");
    // A folder with no plugin in it.
    let err = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "url": url, "folder": "nothing" })).await.unwrap_err();
    assert!(err.contains("no zeron-plugin.json"), "{err}");
}

#[tokio::test]
async fn plugins_uninstall_removes_its_folder_decision_storage_and_update() {
    let f = fixture();
    let fake = Arc::new(FakeDirectory::default());
    let ada = someone(&f, &fake, "ada");
    let path = package_file(&f, "focus.zeron-plugin", &signed_package(&ada, "focus", "1.0.0", &["ui.add"], "return 1"));
    let installed = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    call(&f.plugins, methods::DECIDE_PLUGIN, serde_json::json!({ "id": "focus", "allow": true, "hash": installed["plugin"]["hash"] }))
        .await
        .unwrap();
    let storage = f.plugins.plugins_dir().join(".storage");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(storage.join("focus.json"), "{}").unwrap();
    std::fs::write(&path, signed_package(&ada, "focus", "1.1.0", &["ui.add"], "return 2")).unwrap();
    call(&f.plugins, methods::CHECK_PLUGIN_UPDATES, serde_json::json!({})).await.unwrap();
    assert!(install::found_package_path(f.plugins.plugins_dir(), "focus").is_file());

    call(&f.plugins, methods::REMOVE_PLUGIN, serde_json::json!({ "id": "focus" })).await.unwrap();
    assert!(!f.plugins.plugins_dir().join("focus").exists());
    assert!(!storage.join("focus.json").exists());
    assert!(!install::found_package_path(f.plugins.plugins_dir(), "focus").exists());
    assert!(!State::load(f.plugins.plugins_dir()).decisions.contains_key("focus"));
    let list = call(&f.plugins, methods::LIST_PLUGINS, serde_json::json!({})).await.unwrap();
    assert!(list["plugins"].as_array().unwrap().iter().all(|p| p["id"] != "focus"));
    // Installed again, it asks again.
    let again = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    assert_eq!(again["plugin"]["status"], "pending");
}

#[tokio::test]
async fn plugins_export_packs_a_plugin_that_installs_elsewhere() {
    let f = fixture();
    call(&f.plugins, methods::NEW_PLUGIN, serde_json::json!({ "id": "hello", "name": "Hello", "permissions": ["ui.add"] }))
        .await
        .unwrap();
    let out = f.data.join("out");
    let exported = call(&f.plugins, methods::EXPORT_PLUGIN, serde_json::json!({ "id": "hello", "to": out })).await.unwrap();
    let path = PathBuf::from(exported["path"].as_str().unwrap());
    assert_eq!(path, out.join("hello-0.1.0.zeron-plugin"));
    let package = install::parse(std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(package.key, signing::load_key(&f.data).unwrap().public());
    // On another device of someone else's.
    let g = fixture();
    let there = call(&g.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap();
    assert_eq!(there["plugin"]["status"], "pending");
    // Over a plugin you're making here, refused.
    let here = call(&f.plugins, methods::INSTALL_PLUGIN, serde_json::json!({ "path": path })).await.unwrap_err();
    assert!(here.contains("making a plugin called hello"), "{here}");
}
