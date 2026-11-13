//! Who may decide (R6, R7, R16, R25): regression tests for each attack, and
//! the consent window's way of asking for a review, shared with the other
//! plugin tests.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use zeron_rpc::{Caller, RpcError, RpcReply, methods, with_caller};

use super::*;

/// `params` as Zeron's consent window sends them: an allowing
/// `DecidePlugin` or a `SignPlugin` without a review gets one minted for
/// the plugin as listed now (what consent.rs does before every decision).
/// Everything else passes as is.
pub(super) async fn as_consent_window(plugins: &Plugins, method: &str, mut params: Value) -> Value {
    let allowing = method == methods::DECIDE_PLUGIN && params["allow"] == json!(true);
    if !(allowing || method == methods::SIGN_PLUGIN) || params.get("review").is_some() {
        return params;
    }
    let id = params["id"].as_str().map(str::to_owned).or_else(|| {
        params["path"]
            .as_str()
            .and_then(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });
    let Some(id) = id else { return params };
    plugins.rescan().await;
    let Some(hash) = plugins.entry(&id).map(|e| e.item.hash) else {
        return params;
    };
    if let Ok(RpcReply::Value(minted)) = plugins
        .dispatch(methods::REVIEW_PLUGIN, json!({ "id": id, "hash": hash }))
        .await
    {
        params["review"] = minted["review"].clone();
    }
    params
}

struct Fixture {
    _dir: tempfile::TempDir,
    plugins: Plugins,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), "dev-1");
    std::fs::create_dir_all(plugins.plugins_dir()).unwrap();
    Fixture { _dir: dir, plugins }
}

/// An unsigned plugin asking for `native` and `commands`: it waits for the
/// person.
async fn waiting(plugins: &Plugins, id: &str) -> String {
    let dir = plugins.plugins_dir().join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("init.luau"), "return 1").unwrap();
    let manifest = json!({
        "id": id, "name": id, "version": "1.0.0", "entry": "init.luau",
        "permissions": ["native", "commands"], "commands": ["sh"],
    });
    std::fs::write(dir.join(MANIFEST_FILE), signing::pretty(&manifest)).unwrap();
    plugins.rescan().await;
    let entry = plugins.entry(id).unwrap();
    assert_eq!(entry.item.status, Status::Pending, "{id} waits for the person");
    entry.item.hash
}

async fn call(plugins: &Plugins, method: &str, params: Value) -> Result<Value, String> {
    match plugins.dispatch(method, params).await {
        Ok(RpcReply::Value(value)) => Ok(value),
        Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
        Err(RpcError::Failed(message)) => Err(message),
        Err(other) => Err(other.to_string()),
    }
}

async fn status(plugins: &Plugins, id: &str) -> Status {
    plugins.rescan().await;
    plugins.entry(id).unwrap().item.status
}

/// R6: the IPC token alone (what any local client holding it can send)
/// can't allow a plugin: DecidePlugin needs the consent window's review,
/// for that plugin as shown, once.
#[tokio::test]
async fn an_ipc_token_holder_cannot_allow_a_plugin_without_the_consent_window() {
    let f = fixture();
    let hash = waiting(&f.plugins, "evil").await;

    for review in [None, Some("".to_owned()), Some("ab".repeat(32))] {
        let mut params = json!({ "id": "evil", "allow": true, "hash": hash });
        if let Some(review) = review {
            params["review"] = json!(review);
        }
        let refused = call(&f.plugins, methods::DECIDE_PLUGIN, params).await;
        assert!(refused.as_ref().is_err_and(|e| e.starts_with("review:")), "{refused:?}");
        assert_eq!(status(&f.plugins, "evil").await, Status::Pending);
    }

    // A review for another plugin, or for another version of this one,
    // doesn't carry over.
    let other = waiting(&f.plugins, "other").await;
    let for_other = call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "other", "hash": other }))
        .await
        .unwrap()["review"]
        .clone();
    let refused = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        json!({ "id": "evil", "allow": true, "hash": hash, "review": for_other }),
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(
        call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": "0".repeat(64) }))
            .await
            .is_err(),
        "a review is for the plugin as it is"
    );

    // The consent window's way: a review for it as shown, used once.
    let review = call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": hash }))
        .await
        .unwrap()["review"]
        .clone();
    let allowed = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        json!({ "id": "evil", "allow": true, "hash": hash, "review": review }),
    )
    .await
    .unwrap();
    assert_eq!(allowed["status"], "on", "{allowed}");
    assert_eq!(status(&f.plugins, "evil").await, Status::On);

    // Declining never needs one.
    let declined = call(&f.plugins, methods::DECIDE_PLUGIN, json!({ "id": "other", "allow": false, "hash": other }))
        .await
        .unwrap();
    assert_eq!(declined["status"], "off", "{declined}");
}

/// R6: a used review can't be replayed, even for the same plugin.
#[tokio::test]
async fn a_review_allows_once() {
    let f = fixture();
    let hash = waiting(&f.plugins, "focus").await;
    let review = call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "focus", "hash": hash }))
        .await
        .unwrap()["review"]
        .clone();
    let decide = json!({ "id": "focus", "allow": true, "hash": hash, "review": review });
    call(&f.plugins, methods::DECIDE_PLUGIN, decide.clone()).await.unwrap();
    call(&f.plugins, methods::DECIDE_PLUGIN, json!({ "id": "focus", "allow": false, "hash": hash }))
        .await
        .unwrap();
    let replayed = call(&f.plugins, methods::DECIDE_PLUGIN, decide).await;
    assert!(replayed.is_err(), "{replayed:?}");
    assert_eq!(status(&f.plugins, "focus").await, Status::Off);
}

/// R6: an agent (its MCP server's scoped token) or another device can't
/// mint a review, nor use one minted for the app, nor sign: the engine
/// checks who asked even past the transport guards.
#[tokio::test]
async fn agents_and_other_devices_cannot_review_allow_or_sign() {
    let f = fixture();
    let hash = waiting(&f.plugins, "evil").await;
    for caller in [Caller::Agent { chat: Some("chat-1".into()) }, Caller::Peer { device: None }] {
        let minted = with_caller(
            caller.clone(),
            call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": hash })),
        )
        .await;
        assert!(minted.is_err(), "{caller:?} minted a review: {minted:?}");

        // Even holding one (say, read off the app's connection).
        let review = call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": hash }))
            .await
            .unwrap()["review"]
            .clone();
        let allowed = with_caller(
            caller.clone(),
            call(
                &f.plugins,
                methods::DECIDE_PLUGIN,
                json!({ "id": "evil", "allow": true, "hash": hash, "review": review }),
            ),
        )
        .await;
        assert!(allowed.is_err(), "{caller:?} allowed it: {allowed:?}");

        let review = call(&f.plugins, methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": hash }))
            .await
            .unwrap()["review"]
            .clone();
        let signed = with_caller(
            caller.clone(),
            call(&f.plugins, methods::SIGN_PLUGIN, json!({ "id": "evil", "review": review })),
        )
        .await;
        assert!(signed.is_err(), "{caller:?} signed it as yours: {signed:?}");
        assert_eq!(status(&f.plugins, "evil").await, Status::Pending);
        assert!(!f.plugins.entry("evil").unwrap().item.own, "not made yours");
    }
}

/// R4/R6: SignPlugin (which makes a plugin yours) needs the consent
/// window's review too, and never signs a folder outside the plugins folder.
#[tokio::test]
async fn signing_needs_the_consent_window() {
    let f = fixture();
    waiting(&f.plugins, "theirs").await;
    let refused = call(&f.plugins, methods::SIGN_PLUGIN, json!({ "id": "theirs" })).await;
    assert!(refused.as_ref().is_err_and(|e| e.starts_with("review:")), "{refused:?}");
    assert!(!f.plugins.entry("theirs").unwrap().item.own);

    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("ext");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("init.luau"), "return 1").unwrap();
    let manifest = signing::pretty(&json!({ "id": "ext", "name": "ext", "version": "1.0.0", "entry": "init.luau" }));
    std::fs::write(path.join(MANIFEST_FILE), &manifest).unwrap();
    let refused = call(&f.plugins, methods::SIGN_PLUGIN, json!({ "path": path.display().to_string() })).await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(std::fs::read(path.join(MANIFEST_FILE)).unwrap(), manifest, "left unsigned");
}

/// R6: allowing a template package needs ReviewTemplate's review, and an
/// agent can't mint one.
#[tokio::test]
async fn an_ipc_token_holder_cannot_allow_a_template_without_the_consent_window() {
    let f = fixture();
    for params in [
        json!({ "pending": "midnight-1", "allow": true, "hash": "h" }),
        json!({ "pending": "midnight-1", "allow": true, "hash": "h", "review": "ab".repeat(32) }),
    ] {
        let refused = call(&f.plugins, methods::DECIDE_TEMPLATE, params).await;
        assert!(refused.as_ref().is_err_and(|e| e.starts_with("review:")), "{refused:?}");
    }
    let minted = with_caller(
        Caller::Agent { chat: None },
        call(&f.plugins, methods::REVIEW_TEMPLATE, json!({ "pending": "midnight-1", "hash": "h" })),
    )
    .await;
    assert!(minted.is_err(), "{minted:?}");
    // With one, the decision reaches the package (there's none here).
    let review = call(&f.plugins, methods::REVIEW_TEMPLATE, json!({ "pending": "midnight-1", "hash": "h" }))
        .await
        .unwrap()["review"]
        .clone();
    let reached = call(
        &f.plugins,
        methods::DECIDE_TEMPLATE,
        json!({ "pending": "midnight-1", "allow": true, "hash": "h", "review": review }),
    )
    .await;
    assert!(reached.is_err_and(|e| !e.starts_with("review:")));
}

/// R7: an agent's UseTemplate switches, never approves: the reply says it's
/// a preview, and who asked is kept for the app's bar.
#[tokio::test]
async fn an_agents_use_template_is_a_preview_with_who_asked() {
    let f = fixture();
    let dir = f._dir.path().join(TEMPLATES_DIR);
    let zeron = dir.join(templates::ZERON_DIR);
    std::fs::create_dir_all(&zeron).unwrap();
    std::fs::write(zeron.join("app.luau"), "return nil").unwrap();
    std::fs::write(zeron.join(templates::MANIFEST), r#"{"name":"Zeron"}"#).unwrap();
    std::fs::write(zeron.join(templates::HASHES), "{}").unwrap();
    let mine = dir.join("agent-made");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join(templates::MANIFEST), r#"{"name":"Agent made"}"#).unwrap();

    let used = with_caller(
        Caller::Agent { chat: Some("chat-7".into()) },
        call(&f.plugins, methods::USE_TEMPLATE, json!({ "id": "agent-made" })),
    )
    .await
    .unwrap();
    assert_eq!(used["preview"], true, "{used}");
    assert_eq!(used["by"]["chat"], "chat-7", "{used}");
    let record: Value = serde_json::from_slice(&std::fs::read(dir.join(templates::ACTIVE_BY)).unwrap()).unwrap();
    assert_eq!((record["id"].as_str(), record["by"]["chat"].as_str()), (Some("agent-made"), Some("chat-7")));

    // The app's own switch back clears the record.
    let back = call(&f.plugins, methods::USE_TEMPLATE, json!({ "id": "zeron" })).await.unwrap();
    assert_eq!(back["preview"], false, "{back}");
    assert!(back.get("by").is_none());
    assert!(!dir.join(templates::ACTIVE_BY).exists());
}

/// R6 end to end over the engine's IPC: the agent-scoped token (what
/// `zeron mcp` holds) can't review, allow, sign or take the bearer; the
/// app's token can review and allow.
#[tokio::test]
async fn the_agent_token_cannot_allow_over_ipc() {
    struct Service(Plugins);
    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Service {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            self.0.dispatch(method, params).await
        }
    }

    let f = fixture();
    let hash = waiting(&f.plugins, "evil").await;
    let tokens = zeron_rpc::ipc_auth::IpcTokens::generate().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(zeron_rpc::serve_ws_listener_scoped(
        listener,
        Arc::new(Service(f.plugins.clone())),
        tokens.clone(),
    ));

    let agent = zeron_rpc::connect_ws(&url, Some(&tokens.agent)).await.unwrap();
    for (method, params) in [
        (methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": hash })),
        (methods::DECIDE_PLUGIN, json!({ "id": "evil", "allow": true, "hash": hash })),
        (methods::SIGN_PLUGIN, json!({ "id": "evil" })),
        (methods::DECIDE_TEMPLATE, json!({ "pending": "x", "allow": true, "hash": "h" })),
        (methods::UPDATE_PLUGIN, json!({ "id": "evil" })),
        (methods::ACCESS_TOKEN, json!({})),
    ] {
        assert!(agent.call(method, params).await.is_err(), "the agent token reached {method}");
    }
    assert_eq!(status(&f.plugins, "evil").await, Status::Pending);
    // What its tools need still works.
    agent.call(methods::LIST_PLUGINS, json!({})).await.unwrap();

    let app = zeron_rpc::connect_ws(&url, Some(&tokens.full)).await.unwrap();
    let review = app
        .call(methods::REVIEW_PLUGIN, json!({ "id": "evil", "hash": hash }))
        .await
        .unwrap()["review"]
        .clone();
    let allowed = app
        .call(methods::DECIDE_PLUGIN, json!({ "id": "evil", "allow": true, "hash": hash, "review": review }))
        .await
        .unwrap();
    assert_eq!(allowed["status"], "on", "{allowed}");
}

/// R29: an agent can't name where an export goes (anywhere it could write a
/// file the person runs); it exports to Zeron's exports folder only.
#[tokio::test]
async fn an_agent_cannot_choose_where_a_template_is_exported() {
    let f = fixture();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("x.zeron-template");
    for caller in [Caller::Agent { chat: None }, Caller::Peer { device: None }] {
        let refused = with_caller(
            caller,
            call(
                &f.plugins,
                methods::EXPORT_TEMPLATE,
                json!({ "id": "midnight", "path": path.display().to_string() }),
            ),
        )
        .await;
        assert!(refused.as_ref().is_err_and(|e| e.contains("exports folder")), "{refused:?}");
    }
    assert!(!path.exists());
}

/// R29 for plugins: an agent's export goes to Zeron's plugin-exports folder,
/// never a path of its choosing.
#[tokio::test]
async fn an_agent_exports_a_plugin_only_to_the_exports_folder() {
    let f = fixture();
    call(&f.plugins, methods::NEW_PLUGIN, json!({ "id": "hello", "name": "Hello" })).await.unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("x.zeron-plugin");
    for caller in [Caller::Agent { chat: None }, Caller::Peer { device: None }] {
        let refused = with_caller(
            caller,
            call(&f.plugins, methods::EXPORT_PLUGIN, json!({ "id": "hello", "to": path.display().to_string() })),
        )
        .await;
        assert!(refused.as_ref().is_err_and(|e| e.contains("plugin-exports folder")), "{refused:?}");
    }
    assert!(!path.exists());
    let exported = with_caller(Caller::Agent { chat: None }, call(&f.plugins, methods::EXPORT_PLUGIN, json!({ "id": "hello" })))
        .await
        .unwrap();
    let saved = Path::new(exported["path"].as_str().unwrap());
    assert_eq!(saved, f._dir.path().join("plugin-exports").join("hello-0.1.0.zeron-plugin"));
    assert_eq!(exported["bytes"].as_u64().unwrap(), std::fs::metadata(saved).unwrap().len());
}

/// R14: a device-room dial's proof is this device's registered key over the
/// edge's message, and nothing at all without a key.
#[test]
fn a_dial_proof_is_the_device_key_over_the_link_message() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    assert!(signing::device_link_proof(dir.path(), "dev-b", "c1", 5).is_none(), "no key, no proof");
    let key = signing::load_or_create_key(dir.path()).unwrap();
    let (public, signature) = signing::device_link_proof(dir.path(), "dev-b", "c1", 5).unwrap();
    assert_eq!(public, key.public());
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let verifier = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, b64.decode(&public).unwrap());
    let message = zeron_rpc::caller_proof_message("dev-b", "c1", 5);
    verifier.verify(message.as_bytes(), &b64.decode(&signature).unwrap()).unwrap();
    assert!(verifier.verify(b"zeron-device-link:v1:dev-c:c1:5", &b64.decode(&signature).unwrap()).is_err());
}
