//! Plugins' agent controls through the plugins module: a headless service
//! reacting to an event with no window open, the engine refusing what a
//! plugin wasn't allowed here, your own plugins allowed the controls as
//! any other permission, windows told over their channel, and another device's calls
//! decided by this device's grants.

use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;

use super::agents::tests::{Host, account, chat};
use super::*;

const ROTATE: &str = r#"
local zeron = require("./zeron")("svc")

-- Below 10% left on the active Claude Code account: the next saved one.
zeron.on("usage.threshold", function(e)
	local accounts = zeron.accounts({ provider = e.provider }):await()
	for _, a in accounts do
		if not a.active and a.switchable then
			local done = zeron.switch_account({ provider = e.provider, account = a.account, when_idle = true }):await()
			zeron.emit("switched", { to = a.account, status = done.status })
			return
		end
	end
end, { below = 10, provider = "claude-code" })

zeron.serve("usage", function()
	return zeron.usage({ provider = "claude-code" }):await()
end, { safe = true })

zeron.serve("switch", function(args)
	return zeron.switch_account({ provider = "claude-code", account = args.account }):await()
end)

zeron.serve("models", function()
	return zeron.models("claude-code"):await()
end, { safe = true })
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    plugins: Plugins,
    host: Arc<Host>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), "dev-1");
    std::fs::create_dir_all(plugins.plugins_dir()).unwrap();
    let host = Arc::new(Host::default());
    *lock(&host.snapshot) = zeron_proto::AgentAccountsSnapshot {
        accounts: vec![account("a1", true, 0.5), account("a2", false, 0.1)],
        warnings: vec![],
    };
    *lock(&host.chats) = vec![chat("c1", "opus")];
    plugins.agents().set_host(host.clone());
    Fixture { _dir: dir, plugins, host }
}

async fn call(plugins: &Plugins, method: &str, params: serde_json::Value) -> serde_json::Value {
    let params = super::authority_tests::as_consent_window(plugins, method, params).await;
    match plugins.dispatch(method, params).await {
        Ok(RpcReply::Value(value)) => value,
        Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
        Err(error) => json!({ "rpc_error": error.to_string() }),
    }
}

/// Plugin `svc`, signed as yours, asking for `permissions`.
async fn write(plugins: &Plugins, permissions: &[&str], service: Option<&str>) {
    let dir = plugins.plugins_dir().join("svc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("init.luau"), "return nil").unwrap();
    std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
    let mut manifest = json!({ "id": "svc", "name": "Svc", "version": "1.0.0", "entry": "init.luau", "permissions": permissions });
    if let Some(service) = service {
        std::fs::write(dir.join("service.luau"), service).unwrap();
        manifest["service"] = json!("service.luau");
    }
    std::fs::write(dir.join(MANIFEST_FILE), signing::pretty(&manifest)).unwrap();
    let signed = call(plugins, methods::SIGN_PLUGIN, json!({ "id": "svc" })).await;
    assert!(signed.get("rpc_error").is_none(), "{signed}");
}

async fn status(plugins: &Plugins) -> String {
    call(plugins, methods::LIST_PLUGINS, json!({})).await["plugins"][0]["status"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

async fn allow(plugins: &Plugins) {
    let item = call(plugins, methods::LIST_PLUGINS, json!({})).await["plugins"][0].clone();
    let decided = call(plugins, methods::DECIDE_PLUGIN, json!({ "id": "svc", "allow": true, "hash": item["hash"] })).await;
    assert_eq!(decided["status"], "on", "{decided}");
}

async fn ask(plugins: &Plugins, method: &str, args: serde_json::Value) -> serde_json::Value {
    call(plugins, methods::CALL_PLUGIN_SERVICE, json!({ "plugin": "svc", "method": method, "args": args })).await
}

fn snapshot(f: &Fixture) -> zeron_proto::AgentAccountsSnapshot {
    lock(&f.host.snapshot).clone()
}

fn set_used(f: &Fixture, used: f32) {
    for a in lock(&f.host.snapshot).accounts.iter_mut().filter(|a| a.active) {
        a.usage_windows[0].used_fraction = used;
    }
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..300 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never: {what}");
}

const ALL: &[&str] = &["service", "usage.read", "accounts.read", "accounts.switch", "models.read"];

#[test]
fn agents_your_own_plugin_is_allowed_the_controls_as_any_permission() {
    // What installing your own plugin grants without asking
    // (`inside_sandbox`, lifecycle.rs): the reads and the controls alike.
    let manifest = manifest::Manifest::from_value(json!({
        "id": "svc", "name": "Svc", "version": "1.0.0", "entry": "init.luau",
        "permissions": ["ui.add", "usage.read", "accounts.read", "models.read", "accounts.switch", "models.set"],
    }))
    .unwrap();
    assert_eq!(manifest.problem(), None);
    let granted = capability::inside_sandbox(&capability::record(&manifest));
    assert_eq!(
        granted["permissions"],
        json!(["accounts.read", "accounts.switch", "models.read", "models.set", "ui.add", "usage.read"])
    );
    // And a control added in an update asks again.
    let before = capability::record(
        &manifest::Manifest::from_value(json!({
            "id": "svc", "name": "Svc", "version": "1.0.0", "entry": "init.luau", "permissions": ["usage.read"],
        }))
        .unwrap(),
    );
    let d = capability::diff(&before, &capability::record(&manifest));
    assert!(d.grew() && d.permissions_added.contains(&"accounts.switch".to_string()));
}

#[tokio::test]
async fn agents_a_plugin_waiting_to_be_allowed_gets_nothing() {
    let f = fixture();
    write(&f.plugins, &["ui.add", "usage.read", "accounts.switch"], None).await;
    assert_eq!(status(&f.plugins).await, "pending");
    let refused = call(&f.plugins, methods::PLUGIN_AGENTS, json!({ "plugin": "svc", "op": "usage" })).await;
    assert_eq!(refused["reason"], "not_allowed", "{refused}");
}

#[tokio::test]
async fn agents_a_service_reads_usage_and_is_refused_what_it_was_not_allowed() {
    let f = fixture();
    // Asks for usage only: switching and models are refused by the engine.
    write(&f.plugins, &["service", "usage.read"], Some(ROTATE)).await;
    allow(&f.plugins).await;
    let usage = ask(&f.plugins, "usage", json!({})).await;
    assert_eq!(usage["ok"], true, "{usage}");
    assert_eq!(usage["value"][0]["account"], "a1");
    assert_eq!(usage["value"][0]["windows"][0]["remaining"], 50.0);
    assert!(usage["value"][0].get("email").is_none());
    assert_eq!(f.host.forced.load(Ordering::SeqCst), 0, "reading never probes");
    let switched = ask(&f.plugins, "switch", json!({ "account": "a2" })).await;
    assert_eq!(switched["reason"], "failed", "{switched}");
    assert!(switched["message"].as_str().unwrap().starts_with("not_allowed:"), "{switched}");
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 0);
    let models = ask(&f.plugins, "models", json!({})).await;
    assert!(models["message"].as_str().unwrap().contains("models.read"), "{models}");
}

#[tokio::test]
async fn agents_a_headless_service_switches_when_its_threshold_is_crossed() {
    let f = fixture();
    write(&f.plugins, ALL, Some(ROTATE)).await;
    allow(&f.plugins).await;
    // (Its listeners are known once it has loaded: as a version is first
    // seen, or here, its first call.)
    assert_eq!(ask(&f.plugins, "usage", json!({})).await["ok"], true);
    let state = call(&f.plugins, methods::PLUGIN_STATUS, json!({ "id": "svc" })).await;
    assert_eq!(state["service"]["listeners"][0]["event"], "usage.threshold", "{state}");
    let agents = f.plugins.agents().clone();
    agents.observe_accounts(&snapshot(&f));
    // Over the line, then nearer it: nothing.
    set_used(&f, 0.85);
    agents.observe_accounts(&snapshot(&f));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 0, "no crossing, no switch");
    // Under it: the service, with no window open, switches to a2.
    set_used(&f, 0.95);
    agents.observe_accounts(&snapshot(&f));
    until("the service switched", || f.host.activations.load(Ordering::SeqCst) == 1).await;
    assert!(snapshot(&f).accounts.iter().any(|a| a.id == "a2" && a.active));
    let state = call(&f.plugins, methods::PLUGIN_STATUS, json!({ "id": "svc" })).await;
    assert_eq!(state["agents"]["actions"][0]["op"], "switch_account", "{state}");
}

#[tokio::test]
async fn agents_a_window_hears_its_events_over_its_channel() {
    let f = fixture();
    write(&f.plugins, &["ui.add", "usage.read"], None).await;
    allow(&f.plugins).await;
    let Ok(RpcReply::Stream(mut channel)) = f.plugins.dispatch(methods::WATCH_PLUGIN_CHANNEL, json!({})).await else {
        panic!("a stream");
    };
    let listened = call(
        &f.plugins,
        methods::PLUGIN_AGENTS,
        json!({ "plugin": "svc", "op": "listen", "listeners": [{ "key": "w1", "event": "usage.threshold", "opts": { "below": 10 } }] }),
    )
    .await;
    assert_eq!(listened["ok"], true, "{listened}");
    let agents = f.plugins.agents().clone();
    agents.observe_accounts(&snapshot(&f));
    set_used(&f, 0.97);
    agents.observe_accounts(&snapshot(&f));
    let heard = tokio::time::timeout(Duration::from_secs(5), channel.next()).await.unwrap().unwrap();
    assert_eq!(heard["plugin"], "svc");
    assert_eq!(heard["event"], "zeron:usage.threshold");
    assert_eq!(heard["data"]["listener"], "w1");
    assert_eq!(heard["data"]["data"]["remaining"], 3.0);
    // And a window's ops: its grants, here.
    let usage = call(&f.plugins, methods::PLUGIN_AGENTS, json!({ "plugin": "svc", "op": "usage" })).await;
    assert_eq!(usage["ok"], true, "{usage}");
    let refused = call(&f.plugins, methods::PLUGIN_AGENTS, json!({ "plugin": "svc", "op": "accounts" })).await;
    assert_eq!(refused["reason"], "not_allowed", "{refused}");
}

#[tokio::test]
async fn agents_another_device_is_decided_by_this_devices_grants() {
    let f = fixture();
    let own_key = || signing::load_key(&f.plugins.inner.data_dir).map(|k| k.public());
    let serve = |version: &str, op: &str, key: Option<String>| {
        json!({ "plugin": "svc", "version": version, "authorKey": key, "from": "dev-2", "op": op, "args": { "provider": "claude-code", "account": "a2" } })
    };
    let refused = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "usage", None)).await;
    assert_eq!(refused["reason"], "not_installed", "{refused}");
    write(&f.plugins, &["ui.add", "usage.read", "accounts.switch"], None).await;
    let refused = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "usage", own_key())).await;
    assert_eq!(refused["reason"], "not_approved", "waiting to be allowed here: {refused}");
    allow(&f.plugins).await;
    let refused = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("2.0.0", "usage", own_key())).await;
    assert_eq!(refused["reason"], "version_mismatch", "{refused}");
    let refused = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "usage", None)).await;
    assert_eq!(refused["reason"], "other_author", "{refused}");
    let usage = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "usage", own_key())).await;
    assert_eq!(usage["ok"], true, "{usage}");
    let refused = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "accounts", own_key())).await;
    assert_eq!(refused["reason"], "not_allowed", "not allowed accounts.read here: {refused}");
    let switched = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "switch_account", own_key())).await;
    assert_eq!(switched["ok"], true, "{switched}");
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 1);
    // A window's call names its device; one this device can't reach.
    let offline = call(&f.plugins, methods::PLUGIN_AGENTS, json!({ "plugin": "svc", "op": "usage", "device": "dev-9" })).await;
    assert_eq!(offline["reason"], "offline", "{offline}");
    // The default for new chats is a window's own: never another device's.
    let refused = call(&f.plugins, methods::SERVE_PLUGIN_AGENTS, serve("1.0.0", "set_default_model", own_key())).await;
    assert_eq!(refused["reason"], "bad_call", "{refused}");
}

/// The example (plugins-dev/account-rotate), as it ships: its service, with
/// no window open, switches Claude Code to the next account with more left
/// once the active one drops under 10%, waiting for a running chat.
#[tokio::test]
async fn agents_the_account_rotate_example_switches_once_a_running_chat_finishes() {
    let f = fixture();
    let dir = f.plugins.plugins_dir().join("account-rotate");
    std::fs::create_dir_all(&dir).unwrap();
    let example = "../../apps/zgx/plugins-dev/account-rotate";
    for file in ["zeron-plugin.json", "init.luau", "service.luau"] {
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join(example).join(file), dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
    // A third account, nearly spent: skipped for a2.
    {
        let mut s = lock(&f.host.snapshot);
        let mut spent = account("a0", false, 0.97);
        spent.email = Some("spent@example.com".into());
        s.accounts.insert(1, spent);
    }
    let signed = call(&f.plugins, methods::SIGN_PLUGIN, json!({ "id": "account-rotate" })).await;
    assert!(signed.get("rpc_error").is_none(), "{signed}");
    let item = call(&f.plugins, methods::LIST_PLUGINS, json!({})).await["plugins"][0].clone();
    assert_eq!(item["id"], "account-rotate");
    let decided = call(&f.plugins, methods::DECIDE_PLUGIN, json!({ "id": "account-rotate", "allow": true, "hash": item["hash"] })).await;
    assert_eq!(decided["status"], "on", "{decided}");
    // (Loaded once as its version is first seen: its listeners known.)
    until("its listeners are known", || {
        f.plugins.inner.services.status("account-rotate")["listeners"].as_array().is_some_and(|l| l.len() == 2)
    })
    .await;
    // A chat on Claude Code is running.
    *lock(&f.host.sessions) = vec![super::agents::tests::session("c1", zeron_proto::SessionStatus::Working, None)];
    let agents = f.plugins.agents().clone();
    let sessions = lock(&f.host.sessions).clone();
    agents.observe_sessions(&sessions);
    agents.observe_accounts(&snapshot(&f));
    set_used(&f, 0.93);
    agents.observe_accounts(&snapshot(&f));
    until("its switch is waiting", || !agents.status("account-rotate")["actions"].is_null()).await;
    assert_eq!(agents.status("account-rotate")["actions"][0]["waiting"], true);
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 0, "never under a running chat");
    // The chat finishes: switched, to a2 (a0 has 3% left).
    *lock(&f.host.sessions) = vec![super::agents::tests::session(
        "c1",
        zeron_proto::SessionStatus::Idle,
        Some(zeron_proto::DoneStatus::Completed),
    )];
    let sessions = lock(&f.host.sessions).clone();
    agents.observe_sessions(&sessions);
    until("it switched", || f.host.activations.load(Ordering::SeqCst) == 1).await;
    let active: Vec<String> = snapshot(&f).accounts.iter().filter(|a| a.active).map(|a| a.id.clone()).collect();
    assert_eq!(active, ["a2"]);
}

#[test]
fn agents_permissions_are_known_and_inside_the_sandbox() {
    for p in ["usage.read", "accounts.read", "accounts.switch", "models.read", "models.set"] {
        assert!(manifest::KNOWN_PERMISSIONS.contains(&p), "{p}");
        assert!(!capability::OUTSIDE_SANDBOX.contains(&p), "{p}");
    }
}
