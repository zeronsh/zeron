//! Changing another device's plugins is the person's (the contract's merged
//! build review; the identity lane's probe): an agent's call aimed at
//! another device is refused here, before anything is sent, and so is one
//! another device relays; the person's own goes on to that device.

use serde_json::{Value, json};
use zeron_rpc::{Caller, RpcService, methods, with_caller};

fn core() -> (tempfile::TempDir, zeron_engine::EngineCore) {
    let temp = tempfile::tempdir().unwrap();
    let core = zeron_engine::EngineCore::assemble(
        temp.path(),
        std::sync::Arc::new(zeron_engine::default_registry()),
        zeron_engine::HarnessId::Mock,
        None,
    )
    .unwrap();
    (temp, core)
}

/// `method` with `params`, as `caller` sends it: its error's text.
async fn refusal(
    core: &zeron_engine::EngineCore,
    caller: Caller,
    method: &str,
    params: &Value,
) -> String {
    let rpc = core.rpc_service();
    match with_caller(caller, rpc.handle(method, params.clone())).await {
        Ok(_) => "Ok(reply)".to_owned(),
        Err(error) => error.to_string(),
    }
}

#[tokio::test]
async fn an_agent_never_changes_plugins_on_another_device() {
    let (_temp, core) = core();
    let to = |mut params: Value| {
        params["targetDeviceId"] = json!("laptop");
        params
    };
    let calls = [
        (methods::INSTALL_PLUGIN, to(json!({ "store": "focus" }))),
        (methods::INSTALL_PLUGIN, to(json!({ "reinstall": "focus" }))),
        (
            methods::INSTALL_PLUGIN,
            to(json!({ "url": "https://example.com/focus.zeron-plugin" })),
        ),
        // Saying it's the person's itself changes nothing.
        (
            methods::INSTALL_PLUGIN,
            to(json!({ "store": "focus", "byPerson": true })),
        ),
        (methods::UPDATE_PLUGIN, to(json!({ "id": "focus" }))),
        (
            methods::INSTALL_VERIFIED_PLUGIN,
            to(json!({ "id": "focus", "hash": "ab" })),
        ),
        (methods::REMOVE_PLUGIN, to(json!({ "id": "focus" }))),
        (
            methods::SET_PLUGIN_OFF,
            to(json!({ "id": "focus", "off": true })),
        ),
    ];
    for caller in [
        Caller::Agent {
            chat: Some("chat-1".into()),
        },
        Caller::Agent { chat: None },
    ] {
        for (method, params) in &calls {
            let text = refusal(&core, caller.clone(), method, params).await;
            assert!(text.contains("needs_person"), "{method} {params} -> {text}");
            assert!(
                !text.contains("cannot reach device"),
                "forwarded for an agent: {method} {params} -> {text}"
            );
        }
    }
    // Another device's call relayed on (only the person's own app asks
    // that one): refused here too.
    let peer = Caller::Peer {
        device: Some("desk".into()),
    };
    // (Pre-releases and which updates a device takes aren't sent on at
    // all, but another device asking them is refused the same.)
    let settings = [
        (
            methods::SET_PLUGIN_PRERELEASES,
            json!({ "id": "focus", "on": true }),
        ),
        (
            methods::SET_PLUGIN_UPDATES,
            json!({ "id": "focus", "prefer": "newest" }),
        ),
    ];
    for (method, params) in calls.iter().chain(&settings) {
        let text = refusal(&core, peer.clone(), method, params).await;
        assert!(
            text.contains("needs_person"),
            "relayed: {method} {params} -> {text}"
        );
    }
    // The person's own goes on to the device (none can be reached here).
    let text = refusal(&core, Caller::Local, methods::INSTALL_PLUGIN, &calls[0].1).await;
    assert!(!text.contains("needs_person"), "{text}");
}

#[tokio::test]
async fn another_device_s_change_is_taken_only_as_the_person_s() {
    let (_temp, core) = core();
    let peer = Caller::Peer {
        device: Some("desk".into()),
    };
    // Without the person's word from the device that sent it: refused.
    for params in [
        json!({ "id": "focus" }),
        json!({ "id": "focus", "byPerson": false }),
    ] {
        let text = refusal(&core, peer.clone(), methods::REMOVE_PLUGIN, &params).await;
        assert!(text.contains("needs_person"), "{params} -> {text}");
    }
    // With it, the call is this device's to answer (there's no such plugin).
    let text = refusal(
        &core,
        peer,
        methods::REMOVE_PLUGIN,
        &json!({ "id": "focus", "byPerson": true }),
    )
    .await;
    assert!(!text.contains("needs_person"), "{text}");
}
