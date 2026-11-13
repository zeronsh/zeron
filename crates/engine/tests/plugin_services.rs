//! Plugin services across two engines: engine A (device-a) calls its
//! plugin's service on engine B (device-b) over the device links, through an
//! in-memory device room standing in for the edge (each device hosts its own
//! room; clients are routed by path), on scratch data dirs and a random
//! local port.

#![cfg(unix)]
// tungstenite's `accept_hdr_async` callback fixes its Err type.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request as WsRequest, Response as WsResponse};

use zeron_engine::{EngineCore, HarnessId, HarnessRegistry};
use zeron_rpc::{
    DeviceFrameHeader, LinkCache, LinkCacheConfig, RpcClient, StaticToken, decode_device_frame,
    encode_device_frame, methods,
};

// ── A device room per device (route-only, like the edge's DO) ──────────────

#[derive(Default)]
struct Rooms {
    hosts: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
    clients: HashMap<(String, String), mpsc::UnboundedSender<Vec<u8>>>,
}

async fn device_rooms() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let rooms = Arc::new(Mutex::new(Rooms::default()));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let rooms = rooms.clone();
            tokio::spawn(async move {
                let mut uri = String::new();
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, |req: &WsRequest, res: WsResponse| {
                    uri = req.uri().to_string();
                    Ok(res)
                })
                .await
                else {
                    return;
                };
                let (path, query) = uri.split_once('?').unwrap_or((&uri, ""));
                let device = path.trim_start_matches("/device/").trim_end_matches("/ws").to_string();
                let is_host = query.contains("role=host");
                let conn = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("connId="))
                    .unwrap_or("anon")
                    .to_string();
                // The dialing device, as the edge stamps it once its proof
                // checks out (this stand-in takes the dial's word).
                let caller = query.split('&').find_map(|kv| kv.strip_prefix("callerDevice=")).map(str::to_owned);
                let (mut sink, mut incoming) = ws.split();
                let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                {
                    let mut rooms = rooms.lock().unwrap();
                    if is_host {
                        rooms.hosts.insert(device.clone(), tx);
                    } else {
                        rooms.clients.insert((device.clone(), conn.clone()), tx);
                    }
                }
                let writer = tokio::spawn(async move {
                    while let Some(bytes) = rx.recv().await {
                        if sink.send(WsMessage::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = incoming.next().await {
                    let WsMessage::Binary(bytes) = message else { continue };
                    let Ok((header, payload)) = decode_device_frame(&bytes) else { break };
                    let rooms = rooms.lock().unwrap();
                    if is_host {
                        let Some(to) = header.to else { continue };
                        if let Some(client) = rooms.clients.get(&(device.clone(), to)) {
                            let stripped = DeviceFrameHeader::new(header.s, header.k);
                            let _ = client.send(encode_device_frame(&stripped, &payload).unwrap());
                        }
                    } else if let Some(host) = rooms.hosts.get(&device) {
                        let mut routed = DeviceFrameHeader::new(header.s, header.k);
                        routed.from = Some(conn.clone());
                        routed.from_device = caller.clone();
                        let _ = host.send(encode_device_frame(&routed, &payload).unwrap());
                    }
                }
                writer.abort();
            });
        }
    });
    url
}

// ── Engines ─────────────────────────────────────────────────────────────────

struct Engine {
    core: EngineCore,
    client: RpcClient,
    _host: zeron_rpc::HostRelay,
}

fn engine(dir: &Path, device: &str, rooms: &str) -> Engine {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("device-id"), device).unwrap();
    let core = EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None)
        .expect("engine assembles");
    let host = core.start_host_relay(rooms);
    let mut links = LinkCacheConfig::new(rooms.to_string(), Arc::new(StaticToken("test-user".into())));
    links.probe_timeout = Duration::from_secs(5);
    // Each dial says which device it is (the edge verifies the proof).
    let me = device.to_owned();
    links.caller_proof = Some(Arc::new(move |_: &str, _: &str, _: i64| {
        Some(zeron_rpc::CallerProof { device: me.clone(), key: "test-key".into(), signature: "test-sig".into() })
    }));
    core.set_links(LinkCache::new(links));
    let client = zeron_rpc::memory_client(core.rpc_service());
    Engine { core, client, _host: host }
}

const SERVICE: &str = r#"
local zeron = require("./zeron")("hello-devices")

zeron.serve("where", function(args, call)
	local ran = zeron.run("echo", { zeron.device.id }):await()
	return { device = zeron.device.id, from = call.from.id, echoed = ran.stdout }
end, { safe = true })

zeron.serve("shout", function(args)
	zeron.emit("shouted", { word = args.word, at = zeron.device.id })
	return true
end)
"#;

/// Its icon: listed with it, so another device's window can draw it.
const ICON: &str = r#"<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="currentColor"/></svg>"#;

/// The plugin, installed in `dir`'s engine as `version`, signed as yours.
async fn install(e: &Engine, dir: &Path, version: &str) {
    install_asking(e, dir, version, &[]).await;
}

/// As `install`, asking for `more` besides.
async fn install_asking(e: &Engine, dir: &Path, version: &str, more: &[&str]) {
    let folder = dir.join("plugins").join("hello-devices");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("init.luau"), "return nil").unwrap();
    std::fs::write(folder.join("service.luau"), SERVICE).unwrap();
    std::fs::write(folder.join("zeron.luau"), zeron_engine::plugins::scaffold::SDK).unwrap();
    std::fs::write(folder.join("icon.svg"), ICON).unwrap();
    let permissions: Vec<&str> = ["service", "commands"].into_iter().chain(more.iter().copied()).collect();
    let manifest = json!({
        "id": "hello-devices", "name": "Hello devices", "version": version, "entry": "init.luau",
        "service": "service.luau",
        "permissions": permissions,
        "commands": ["echo"],
        "icon": "icon.svg",
    });
    std::fs::write(folder.join("zeron-plugin.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    // As Zeron's consent window signs: with a review for it as listed.
    let review = review(e, "hello-devices").await;
    let signed = e
        .client
        .call(methods::SIGN_PLUGIN, json!({ "id": "hello-devices", "review": review }))
        .await
        .unwrap();
    assert_eq!(signed["version"], version, "{signed}");
}

/// A consent-window review for plugin `id` as `e` lists it now.
async fn review(e: &Engine, id: &str) -> Value {
    let list = e.client.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    let item = list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap();
    let minted = e
        .client
        .call(methods::REVIEW_PLUGIN, json!({ "id": id, "hash": item["hash"] }))
        .await
        .unwrap();
    minted["review"].clone()
}

async fn allow(e: &Engine) {
    let list = e.client.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    let item = list["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "hello-devices")
        .cloned()
        .unwrap();
    let review = review(e, "hello-devices").await;
    let decided = e
        .client
        .call(
            methods::DECIDE_PLUGIN,
            json!({ "id": "hello-devices", "allow": true, "hash": item["hash"], "review": review }),
        )
        .await
        .unwrap();
    assert_eq!(decided["status"], "on", "{decided}");
}

async fn ask(e: &Engine, device: &str, method: &str, args: Value) -> Value {
    e.client
        .call(
            methods::CALL_PLUGIN_SERVICE,
            json!({ "plugin": "hello-devices", "device": device, "method": method, "args": args }),
        )
        .await
        .unwrap()
}

/// The first answer that isn't `offline` (the host relays dial with backoff).
async fn ask_when_up(e: &Engine, device: &str, method: &str, args: Value) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let reply = ask(e, device, method, args.clone()).await;
        if reply["reason"] != "offline" || tokio::time::Instant::now() > deadline {
            return reply;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_window_on_one_device_calls_its_service_on_another() {
    let rooms = device_rooms().await;
    let dirs = tempfile::tempdir().unwrap();
    let (dir_a, dir_b) = (dirs.path().join("a"), dirs.path().join("b"));
    // One person's two devices: the same plugin key (the same author).
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    zeron_engine::plugins::signing::load_or_create_key(&dir_a).unwrap();
    std::fs::copy(
        dir_a.join(zeron_engine::plugins::signing::KEY_FILE),
        dir_b.join(zeron_engine::plugins::signing::KEY_FILE),
    )
    .unwrap();
    let a = engine(&dir_a, "device-a", &rooms);
    let b = engine(&dir_b, "device-b", &rooms);


    // Not on B yet.
    install(&a, &dir_a, "1.0.0").await;
    allow(&a).await;
    let refused = ask_when_up(&a, "device-b", "where", json!({})).await;
    assert_eq!(refused["reason"], "not_installed", "{refused}");

    // On B, but B hasn't allowed it: approvals are per device.
    install(&b, &dir_b, "1.0.0").await;
    let refused = ask(&a, "device-b", "where", json!({})).await;
    assert_eq!(refused["reason"], "not_approved", "{refused}");

    // A lists B's (Settings → Plugins' device switcher). An allow sent
    // there needs a review B minted for A (one_device_allows_… below):
    // without one, or with one A minted for itself, it's refused and
    // changes nothing there. Then B allows it, and B's service answers,
    // running B's own `echo`.
    let on_b = a
        .client
        .call(methods::LIST_PLUGINS, json!({ "targetDeviceId": "device-b" }))
        .await
        .unwrap();
    let item = on_b["plugins"].as_array().unwrap().iter().find(|p| p["id"] == "hello-devices").cloned().unwrap();
    assert_eq!(item["status"], "pending", "B lists its own: {item}");
    assert_eq!(item["ask"], json!(["commands", "service"]), "{item}");
    // Its icon is B's file, carried in the listing (A can't read B's disk).
    assert_eq!(item["icon_svg"], ICON, "{item}");
    for review in [Value::Null, review(&a, "hello-devices").await] {
        let forwarded = a
            .client
            .call(
                methods::DECIDE_PLUGIN,
                json!({ "targetDeviceId": "device-b", "id": "hello-devices", "allow": true, "hash": item["hash"], "review": review }),
            )
            .await;
        assert!(forwarded.is_err(), "allowed without B's review: {forwarded:?}");
    }
    let list = b.client.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    let still = list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == "hello-devices").cloned().unwrap();
    assert_eq!(still["status"], "pending", "{still}");
    allow(&b).await;
    // (A file here can't be installed there.)
    let refused = a
        .client
        .call(methods::INSTALL_PLUGIN, json!({ "targetDeviceId": "device-b", "path": "/tmp/x.zeron-plugin" }))
        .await;
    assert!(refused.is_err(), "{refused:?}");
    let answered = ask(&a, "device-b", "where", json!({})).await;
    assert_eq!(
        answered,
        json!({ "ok": true, "value": { "device": "device-b", "from": "device-a", "echoed": "device-b\n" } }),
    );
    // And A's own, here.
    let here = ask(&a, "", "where", json!({})).await;
    assert_eq!(here["value"]["device"], "device-a", "{here}");

    // A's window watches B's service: B's events reach A's windows.
    let mut channel = a.client.subscribe_scoped(methods::WATCH_PLUGIN_CHANNEL, json!({})).await.unwrap();
    assert_eq!(ask(&a, "device-b", "@watch", Value::Null).await["ok"], true);
    // (Until A's channel is subscribed, an event has nowhere to land: shout
    // again until one does.)
    let mut event = None;
    for _ in 0..20 {
        assert_eq!(ask(&a, "device-b", "shout", json!({ "word": "hey" })).await["ok"], true);
        if let Ok(Some(heard)) = tokio::time::timeout(Duration::from_millis(500), channel.recv()).await {
            event = Some(heard);
            break;
        }
    }
    let event = event.expect("B's event reached A");
    assert_eq!(event["event"], "shouted", "{event}");
    assert_eq!(event["from"]["id"], "device-b", "{event}");
    assert_eq!(event["data"], json!({ "word": "hey", "at": "device-b" }));

    // B updates: another version than A's is refused, saying so.
    install(&b, &dir_b, "1.1.0").await;
    let refused = ask(&a, "device-b", "where", json!({})).await;
    assert_eq!(refused["reason"], "version_mismatch", "{refused}");

    // Turned off on B.
    install(&b, &dir_b, "1.0.0").await;
    let off = a
        .client
        .call(methods::SET_PLUGIN_OFF, json!({ "targetDeviceId": "device-b", "id": "hello-devices", "off": true }))
        .await
        .unwrap();
    assert_eq!(off["status"], "off", "{off}");
    assert_eq!(ask(&a, "device-b", "where", json!({})).await["reason"], "off");

    // A device that isn't up.
    let refused = ask(&a, "device-z", "where", json!({})).await;
    assert!(
        refused["reason"] == "offline" || refused["reason"] == "timeout",
        "{refused}"
    );
    // Its plugins can't be listed: an error, which Settings → Plugins says
    // as the device being offline.
    let listed = a.client.call(methods::LIST_PLUGINS, json!({ "targetDeviceId": "device-z" })).await;
    assert!(format!("{listed:?}").contains("unreachable"), "{listed:?}");

    a.core.shutdown().await;
    b.core.shutdown().await;
}

// ── Your plugins on your other devices ──────────────────────────────────────

/// Plugin `id` as `e` lists it on `device` (none: its own).
async fn listed(e: &Engine, device: Option<&str>, id: &str) -> Option<Value> {
    let params = match device {
        Some(device) => json!({ "targetDeviceId": device }),
        None => json!({}),
    };
    let list = e.client.call(methods::LIST_PLUGINS, params).await.unwrap();
    list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned()
}

/// As A's consent window does for a plugin on `device`: a review minted
/// there, for it as listed there.
async fn review_on(e: &Engine, device: &str, item: &Value) -> Result<Value, String> {
    e.client
        .call(methods::REVIEW_PLUGIN, json!({ "targetDeviceId": device, "id": item["id"], "hash": item["hash"] }))
        .await
        .map(|minted| minted["review"].clone())
        .map_err(|e| e.to_string())
}

async fn allow_on(e: &Engine, device: &str, item: &Value, review: &Value) -> Result<Value, String> {
    e.client
        .call(
            methods::DECIDE_PLUGIN,
            json!({ "targetDeviceId": device, "id": item["id"], "allow": true, "hash": item["hash"], "review": review }),
        )
        .await
        .map_err(|e| e.to_string())
}

/// As Settings → Plugins sends a `.zeron-plugin` file on this device to
/// `device`: in pieces (small ones here, so there are several), then
/// installed there.
async fn send_file(e: &Engine, device: &str, file: &Path) -> Result<Value, String> {
    use base64::Engine as _;
    let raw = std::fs::read(file).unwrap();
    let mut upload = Value::Null;
    let mut pieces = 0;
    for piece in raw.chunks(2048) {
        let staged = e
            .client
            .call(
                methods::STAGE_PLUGIN_PACKAGE,
                json!({ "targetDeviceId": device, "upload": upload, "data": base64::engine::general_purpose::STANDARD.encode(piece) }),
            )
            .await
            .map_err(|e| e.to_string())?;
        upload = staged["upload"].clone();
        pieces += 1;
    }
    assert!(pieces > 1, "sent in pieces");
    e.client
        .call(
            methods::INSTALL_PLUGIN,
            json!({ "targetDeviceId": device, "upload": upload, "name": file.file_name().unwrap().to_string_lossy() }),
        )
        .await
        .map_err(|e| e.to_string())
}

/// `e`'s plugin as a `.zeron-plugin` file in `dir`.
async fn export(e: &Engine, dir: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let out = e
        .client
        .call(methods::EXPORT_PLUGIN, json!({ "id": "hello-devices", "to": dir }))
        .await
        .unwrap();
    std::path::PathBuf::from(out["path"].as_str().unwrap())
}

/// An agent's call on `e` (its MCP server's scoped token, as the engine
/// sees it once past the IPC gate).
async fn as_agent(e: &Engine, method: &str, params: Value) -> Result<Value, String> {
    use zeron_rpc::RpcService as _;
    let rpc = e.core.rpc_service();
    let reply = zeron_rpc::with_caller(zeron_rpc::Caller::Agent { chat: Some("chat-1".into()) }, async move {
        rpc.handle(method, params).await
    })
    .await;
    match reply {
        Ok(zeron_rpc::RpcReply::Value(value)) => Ok(value),
        Ok(_) => Ok(Value::Null),
        Err(error) => Err(error.to_string()),
    }
}

/// Beta users couldn't install a plugin on another of their devices: its
/// Allow was refused on the way. Now a person installs someone else's
/// plugin on another of their devices from this one (a `.zeron-plugin` file
/// here), sees what it asks as that device reads it, and allows it there;
/// an update asking for more waits there again, and is allowed from here
/// too. The decision is that device's alone (this one hasn't allowed it),
/// held to a review that device minted for exactly what was shown, for
/// this very device, never for an agent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_device_installs_and_allows_a_plugin_on_another() {
    let rooms = device_rooms().await;
    let dirs = tempfile::tempdir().unwrap();
    let (dir_a, dir_b, dir_c) = (dirs.path().join("a"), dirs.path().join("b"), dirs.path().join("c"));
    let a = engine(&dir_a, "device-a", &rooms);
    let b = engine(&dir_b, "device-b", &rooms);
    // Someone else made it (another key): a file they gave you, on A.
    let c = engine(&dir_c, "device-c", &rooms);
    install(&c, &dir_c, "1.0.0").await;
    let file = export(&c, &dirs.path().join("downloads")).await;

    // A sends it to B (once B is up): installed there, waiting for you.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let installed = loop {
        match send_file(&a, "device-b", &file).await {
            Ok(installed) => break installed,
            Err(error) if error.contains("unreachable") && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => panic!("couldn't install it on B: {error}"),
        }
    };
    assert_eq!(installed["waiting"], true, "{installed}");
    assert_eq!(installed["plugin"]["status"], "pending", "{installed}");
    assert!(listed(&a, None, "hello-devices").await.is_none(), "on B, not A");
    // An agent can't send one there.
    let refused = as_agent(&a, methods::STAGE_PLUGIN_PACKAGE, json!({ "targetDeviceId": "device-b", "data": "aGk=" })).await;
    assert!(refused.is_err(), "{refused:?}");

    // A's consent window shows it as B lists it: what B read of it.
    let item = listed(&a, Some("device-b"), "hello-devices").await.unwrap();
    assert_eq!(item["status"], "pending", "{item}");
    assert_eq!(item["ask"], json!(["commands", "service"]), "{item}");
    assert_eq!(item["author"]["kind"], "unknown", "{item}");

    // An agent on A can't mint a review on B, nor allow there.
    let refused = as_agent(&a, methods::REVIEW_PLUGIN, json!({ "targetDeviceId": "device-b", "id": item["id"], "hash": item["hash"] })).await;
    assert!(refused.is_err(), "{refused:?}");
    let review = review_on(&a, "device-b", &item).await.expect("A's window gets a review from B");
    let refused = as_agent(
        &a,
        methods::DECIDE_PLUGIN,
        json!({ "targetDeviceId": "device-b", "id": item["id"], "allow": true, "hash": item["hash"], "review": review }),
    )
    .await;
    assert!(refused.is_err(), "an agent allowed it: {refused:?}");
    // A review B minted for A is A's: B's own app can't spend it either
    // (and a misused one is used up).
    let review = review_on(&a, "device-b", &item).await.unwrap();
    let refused = b
        .client
        .call(methods::DECIDE_PLUGIN, json!({ "id": item["id"], "allow": true, "hash": item["hash"], "review": review }))
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(allow_on(&a, "device-b", &item, &review).await.is_err(), "used up");
    // Nor a review for something other than what B lists.
    let stale = json!({ "id": item["id"], "hash": "ab".repeat(32) });
    assert!(review_on(&a, "device-b", &stale).await.is_err());
    assert_eq!(listed(&b, None, "hello-devices").await.unwrap()["status"], "pending");

    // Your Allow, from A's window: on, on B alone.
    let review = review_on(&a, "device-b", &item).await.unwrap();
    let decided = allow_on(&a, "device-b", &item, &review).await.expect("allowed on B from A");
    assert_eq!(decided["status"], "on", "{decided}");
    let on_b = listed(&b, None, "hello-devices").await.unwrap();
    assert_eq!(on_b["status"], "on", "{on_b}");
    assert_eq!(on_b["allowed"], json!(["commands", "service"]), "{on_b}");
    assert!(listed(&a, None, "hello-devices").await.is_none(), "approvals are per device");

    // Its maker ships 1.1.0, asking for more: sent to B the same way, it
    // waits there again with what it adds, and A allows that too.
    install_asking(&c, &dir_c, "1.1.0", &["links.open"]).await;
    let file = export(&c, &dirs.path().join("downloads-1.1")).await;
    let updated = send_file(&a, "device-b", &file).await.expect("updated on B from A");
    assert_eq!(updated["update"], true, "{updated}");
    let item = listed(&a, Some("device-b"), "hello-devices").await.unwrap();
    assert_eq!(item["status"], "pending", "asks again: {item}");
    assert_eq!(item["version"], "1.1.0", "{item}");
    let review = review_on(&a, "device-b", &item).await.unwrap();
    assert_eq!(allow_on(&a, "device-b", &item, &review).await.unwrap()["status"], "on");
    let on_b = listed(&b, None, "hello-devices").await.unwrap();
    assert_eq!(on_b["allowed"], json!(["commands", "links.open", "service"]), "{on_b}");

    // A device that isn't up says so, as the link does (Settings →
    // Plugins reads either as "Can't reach …", zgx remote::unreachable).
    let away = review_on(&a, "device-z", &item).await;
    assert!(away.as_ref().is_err_and(|e| e.contains("unreachable") || e.contains("timed out")), "{away:?}");

    a.core.shutdown().await;
    b.core.shutdown().await;
    c.core.shutdown().await;
}
