//! Protocol-level tests of the local edge against raw sockets and HTTP: the
//! token gate, chat2 rows/checkpoints, the registry room, the device relay
//! and its nudges — and that every room survives an edge restart.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use zeron_localedge::{LocalEdge, LocalEdgeConfig, LocalEdgeError};
use zeron_rpc::device_room::{CLIENT_CLOSED, CLIENT_GONE, HOST_CLOSED, HOST_OFFLINE, RELAY_KIND};
use zeron_rpc::{DeviceFrameHeader, decode_device_frame, encode_device_frame};
use zeron_sync::chat_frames::{self, WireFrame, frame_type};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start(dir: &std::path::Path, port: u16) -> LocalEdge {
    LocalEdge::start(LocalEdgeConfig::loopback(dir, port, TOKEN))
        .await
        .expect("local edge starts")
}

fn ws_url(edge: &LocalEdge, path: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("ws://{}{path}{sep}token={TOKEN}", edge.addr())
}

async fn dial(edge: &LocalEdge, path: &str) -> Socket {
    tokio_tungstenite::connect_async(ws_url(edge, path))
        .await
        .expect("websocket upgrade")
        .0
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn next_message(socket: &mut Socket) -> Message {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame within 5s")
            .expect("socket open")
            .expect("frame");
        if !matches!(message, Message::Ping(_) | Message::Pong(_)) {
            return message;
        }
    }
}

async fn next_chat(socket: &mut Socket) -> WireFrame {
    match next_message(socket).await {
        Message::Binary(bytes) => chat_frames::decode(&bytes).expect("chat frame"),
        other => panic!("expected a binary chat frame, got {other:?}"),
    }
}

async fn send_chat(socket: &mut Socket, kind: u8, header: Value, payload: &[u8]) {
    socket
        .send(Message::Binary(chat_frames::encode(kind, &header, payload)))
        .await
        .unwrap();
}

async fn next_json(socket: &mut Socket) -> Value {
    match next_message(socket).await {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

async fn next_device(socket: &mut Socket) -> (DeviceFrameHeader, Vec<u8>) {
    match next_message(socket).await {
        Message::Binary(bytes) => decode_device_frame(&bytes).unwrap(),
        other => panic!("expected a device frame, got {other:?}"),
    }
}

async fn send_device(socket: &mut Socket, header: DeviceFrameHeader, payload: &[u8]) {
    socket
        .send(Message::Binary(
            encode_device_frame(&header, payload).unwrap(),
        ))
        .await
        .unwrap();
}

fn relay_code(payload: &[u8]) -> String {
    serde_json::from_slice::<Value>(payload).unwrap()["error"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn every_route_but_health_requires_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path(), 0).await;
    let base = edge.url();

    // Liveness and the public release feed stay open (the engine's and the
    // client's reachability probes and the updater send no bearer).
    let health = http().get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.json::<Value>().await.unwrap()["ok"], true);
    let latest = http()
        .get(format!("{base}/releases/latest.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(latest.status(), 200);
    assert_eq!(
        latest.text().await.unwrap().trim(),
        env!("CARGO_PKG_VERSION")
    );

    let rows = format!("{base}/registry/local/rows");
    for request in [
        http().get(&rows),
        http().get(&rows).bearer_auth("wrong-token-wrong-token"),
        http().get(&rows).bearer_auth(&TOKEN[..TOKEN.len() - 1]),
        http().get(format!("{rows}?token=nope")),
        http().post(format!("{base}/device/d1/nudge")),
        http().get(format!("{base}/blob/c1/p1")),
    ] {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"],
            "unauthenticated"
        );
    }
    // Header and query forms of the right token both pass.
    assert_eq!(
        http()
            .get(&rows)
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http()
            .get(format!("{rows}?token={TOKEN}"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // WebSocket upgrades are refused before the handshake completes.
    for url in [
        format!("ws://{}/chat2/c1/ws", edge.addr()),
        format!("ws://{}/registry/local/ws?token=bad", edge.addr()),
        format!("ws://{}/device/d1/ws?role=host", edge.addr()),
    ] {
        assert!(tokio_tungstenite::connect_async(url).await.is_err());
    }
    let mut request = format!("ws://{}/registry/local/ws", edge.addr())
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_ok());

    // Unknown routes are a plain 404 once authenticated; WorkOS routes answer
    // like a Worker without WorkOS.
    let missing = http()
        .get(format!("{base}/nope"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    let auth = http()
        .post(format!("{base}/auth/refresh"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(auth.status(), 501);
}

#[tokio::test]
async fn weak_tokens_are_refused_and_only_loopback_is_bound() {
    let dir = tempfile::tempdir().unwrap();
    let weak = LocalEdge::start(LocalEdgeConfig::loopback(dir.path(), 0, "short")).await;
    assert!(matches!(weak, Err(LocalEdgeError::InvalidToken)));
    let edge = start(dir.path(), 0).await;
    assert!(edge.addr().ip().is_loopback());
}

#[tokio::test]
async fn chat_rooms_relay_dedupe_checkpoint_and_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path(), 0).await;
    let port = edge.addr().port();
    let base = edge.url();

    let mut host = dial(&edge, "/chat2/chat-1/ws?device=host").await;
    let mut phone = dial(&edge, "/chat2/chat-1/ws?device=phone").await;
    send_chat(
        &mut host,
        frame_type::HELLO,
        json!({ "cursor": 0, "device": "host" }),
        &[],
    )
    .await;
    let state = next_chat(&mut host).await;
    assert_eq!(state.kind, frame_type::STATE);
    assert_eq!(state.header["headSeq"], 0);
    // Rows before hello are refused; push without hello carries the batch id.
    send_chat(
        &mut phone,
        frame_type::PUSH,
        json!({ "batchId": "early" }),
        b"x",
    )
    .await;
    let refused = next_chat(&mut phone).await;
    assert_eq!(refused.kind, frame_type::ERROR);
    assert_eq!(refused.header["batchId"], "early");
    send_chat(
        &mut phone,
        frame_type::HELLO,
        json!({ "cursor": 0, "device": "phone" }),
        &[],
    )
    .await;
    assert_eq!(next_chat(&mut phone).await.kind, frame_type::STATE);
    send_chat(
        &mut phone,
        frame_type::ROWS_REQ,
        json!({ "after": 0, "excludeOwn": false }),
        &[],
    )
    .await;
    assert_eq!(next_chat(&mut phone).await.kind, frame_type::ROWS_DONE);

    // Push → ack to the sender, row to the other socket; a replay is a dup.
    send_chat(
        &mut phone,
        frame_type::PUSH,
        json!({ "batchId": "b1" }),
        b"update-1",
    )
    .await;
    let ack = next_chat(&mut phone).await;
    assert_eq!(
        (
            ack.kind,
            ack.header["seq"].clone(),
            ack.header["dup"].clone()
        ),
        (frame_type::ACK, json!(1), json!(false))
    );
    let row = next_chat(&mut host).await;
    assert_eq!(row.kind, frame_type::ROW);
    assert_eq!(row.header["device"], "phone");
    assert_eq!(row.payload, b"update-1");
    send_chat(
        &mut phone,
        frame_type::PUSH,
        json!({ "batchId": "b1" }),
        b"update-1",
    )
    .await;
    let dup = next_chat(&mut phone).await;
    assert_eq!(
        (dup.header["seq"].clone(), dup.header["dup"].clone()),
        (json!(1), json!(true))
    );

    // HTTP push twin relays to every ready socket.
    let pushed = http()
        .post(format!("{base}/chat2/chat-1/rows?batchId=b2&device=host"))
        .bearer_auth(TOKEN)
        .body(b"update-2".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(pushed.json::<Value>().await.unwrap()["seq"], 2);
    assert_eq!(next_chat(&mut phone).await.payload, b"update-2");
    assert_eq!(next_chat(&mut host).await.payload, b"update-2");

    // excludeOwn skips the requester's own rows.
    send_chat(
        &mut phone,
        frame_type::ROWS_REQ,
        json!({ "after": 0, "excludeOwn": true }),
        &[],
    )
    .await;
    let only = next_chat(&mut phone).await;
    assert_eq!(
        (only.kind, only.header["seq"].clone()),
        (frame_type::ROW, json!(2))
    );
    assert_eq!(next_chat(&mut phone).await.kind, frame_type::ROWS_DONE);
    send_chat(&mut phone, frame_type::PROBE, json!({}), &[]).await;
    assert_eq!(next_chat(&mut phone).await.header["headSeq"], 2);

    // Presence is relayed to the other sockets only.
    send_chat(
        &mut host,
        frame_type::PRESENCE,
        json!({ "at": 7 }),
        b"ephemeral",
    )
    .await;
    let beat = next_chat(&mut phone).await;
    assert_eq!(
        (beat.kind, beat.header["device"].clone()),
        (frame_type::PRESENCE, json!("host"))
    );

    // Checkpoints: absent is a 404, then floor-guarded and Range-resumable.
    let checkpoint = format!("{base}/chat2/chat-1/checkpoint");
    let absent = http()
        .get(&checkpoint)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(absent.status(), 404);
    let empty_frontier = http()
        .post(format!("{checkpoint}?seqCovered=1"))
        .bearer_auth(TOKEN)
        .body(b"snapshot".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(empty_frontier.status(), 400);
    let committed = http()
        .post(format!("{checkpoint}?seqCovered=1"))
        .bearer_auth(TOKEN)
        .header("x-chat2-frontier", "ZnJvbnRpZXI=")
        .body(b"snapshot".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(committed.json::<Value>().await.unwrap()["pruned"], 1);
    let ahead = http()
        .post(format!("{checkpoint}?seqCovered=9"))
        .bearer_auth(TOKEN)
        .header("x-chat2-frontier", "ZnJvbnRpZXI=")
        .body(b"snapshot".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(ahead.status(), 409);
    let resumed = http()
        .get(&checkpoint)
        .bearer_auth(TOKEN)
        .header("range", "bytes=4-")
        .send()
        .await
        .unwrap();
    assert_eq!(resumed.status(), 206);
    assert_eq!(resumed.headers()["x-chat2-checkpoint-seq"], "1");
    assert_eq!(resumed.headers()["content-range"], "bytes 4-7/8");
    assert_eq!(resumed.bytes().await.unwrap().as_ref(), b"shot");
    let past_end = http()
        .get(&checkpoint)
        .bearer_auth(TOKEN)
        .header("range", "bytes=8-")
        .send()
        .await
        .unwrap();
    assert_eq!(past_end.status(), 416);

    // Sidecars are served verbatim.
    http()
        .put(format!("{base}/chat2/chat-1/tail"))
        .bearer_auth(TOKEN)
        .header("content-type", "application/json")
        .body(r#"{"tail":[]}"#)
        .send()
        .await
        .unwrap();

    // Restart on the same port: rows, floor, checkpoint and sidecar persist.
    drop(host);
    drop(phone);
    edge.shutdown().await;
    let edge = start(dir.path(), port).await;
    let pulled = http()
        .get(format!("{base}/chat2/chat-1/rows?after=0&device=phone"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let mut frames = Vec::new();
    let mut rest = pulled.as_ref();
    while rest.len() >= 4 {
        let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        frames.push(chat_frames::decode(&rest[4..4 + len]).unwrap());
        rest = &rest[4 + len..];
    }
    assert_eq!(frames[0].kind, frame_type::STATE);
    assert_eq!(frames[0].header["seqFloor"], 1);
    assert_eq!(frames[0].header["checkpointSize"], 8);
    assert_eq!(frames[0].payload, b"frontier");
    assert_eq!(frames[1].header["seq"], 2);
    assert_eq!(frames[1].payload, b"update-2");
    assert_eq!(frames[2].kind, frame_type::ROWS_DONE);
    let tail = http()
        .get(format!("{base}/chat2/chat-1/tail"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(tail.text().await.unwrap(), r#"{"tail":[]}"#);
    // The batch-id dedupe survived too.
    let replay = http()
        .post(format!("{base}/chat2/chat-1/rows?batchId=b2&device=host"))
        .bearer_auth(TOKEN)
        .body(b"update-2".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(replay.json::<Value>().await.unwrap()["dup"], true);
    drop(edge);
}

#[tokio::test]
async fn registry_merges_broadcasts_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path(), 0).await;
    let port = edge.addr().port();
    let base = edge.url();
    let hlc = |ms: u64| format!("{ms:013}-000000-dev-a");

    let mut engine = dial(&edge, "/registry/local/ws?device=engine").await;
    let mut phone = dial(&edge, "/registry/local/ws?device=phone").await;
    engine
        .send(Message::Text(
            json!({ "t": "hello", "cursor": null, "device": "engine" }).to_string(),
        ))
        .await
        .unwrap();
    let state = next_json(&mut engine).await;
    assert_eq!(
        (
            state["t"].clone(),
            state["seq"].clone(),
            state["full"].clone()
        ),
        (json!("state"), json!(0), json!(true))
    );
    phone
        .send(Message::Text(
            json!({ "t": "hello", "cursor": 0, "device": "phone" }).to_string(),
        ))
        .await
        .unwrap();
    next_json(&mut phone).await;

    engine
        .send(Message::Text(
            json!({ "t": "push", "batch": "b1", "ops": [
                { "kind": "chats", "id": "c1", "op": "upsert", "set": { "title": "one" }, "hlc": hlc(2) },
                { "kind": "chats", "id": "c2", "op": "update", "set": { "title": "never" }, "hlc": hlc(2) }
            ] })
            .to_string(),
        ))
        .await
        .unwrap();
    // Merged rows reach EVERY socket (sender first sees rows, then its ack).
    let rows = next_json(&mut engine).await;
    assert_eq!(rows["t"], "rows");
    assert_eq!(rows["rows"][0]["fields"]["title"], "one");
    let ack = next_json(&mut engine).await;
    assert_eq!(
        (ack["t"].clone(), ack["applied"].clone(), ack["seq"].clone()),
        (json!("ack"), json!(1), json!(1))
    );
    assert_eq!(next_json(&mut phone).await["rows"][0]["id"], "c1");

    // An older write loses LWW: applied 0, no broadcast, seq unchanged.
    phone
        .send(Message::Text(
            json!({ "t": "push", "batch": "b2", "ops": [
                { "kind": "chats", "id": "c1", "op": "update", "set": { "title": "stale" }, "hlc": hlc(1) }
            ] })
            .to_string(),
        ))
        .await
        .unwrap();
    let ack = next_json(&mut phone).await;
    assert_eq!(
        (ack["applied"].clone(), ack["seq"].clone()),
        (json!(0), json!(1))
    );
    // One invalid op rejects the whole batch.
    phone
        .send(Message::Text(
            json!({ "t": "push", "batch": "b3", "ops": [
                { "kind": "chats", "id": "c1", "op": "update", "set": { "title": "x" }, "hlc": hlc(3) },
                { "kind": "Bad", "id": "c1", "op": "update", "set": {}, "hlc": hlc(3) }
            ] })
            .to_string(),
        ))
        .await
        .unwrap();
    let rejected = next_json(&mut phone).await;
    assert_eq!(
        (rejected["t"].clone(), rejected["code"].clone()),
        (json!("error"), json!("invalid_op"))
    );

    // Presence beats go to the others; probes answer with the seq.
    phone
        .send(Message::Text(
            json!({ "t": "presence", "at": 42 }).to_string(),
        ))
        .await
        .unwrap();
    let beat = next_json(&mut engine).await;
    assert_eq!(
        (beat["device"].clone(), beat["at"].clone()),
        (json!("phone"), json!(42))
    );
    phone.send(Message::Text("ping".into())).await.unwrap();
    assert_eq!(next_message(&mut phone).await, Message::Text("pong".into()));

    // HTTP twins: push, then a delta pull.
    let pushed = http()
        .post(format!("{base}/registry/local/push?device=phone"))
        .bearer_auth(TOKEN)
        .json(&json!({ "batch": "b4", "ops": [
            { "kind": "devices", "id": "phone", "op": "upsert", "set": { "name": "Phone" }, "hlc": hlc(5) }
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(pushed.json::<Value>().await.unwrap()["seq"], 2);
    let delta = http()
        .get(format!(
            "{base}/registry/local/rows?since=1&device=phone&beat=1"
        ))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        (delta["full"].clone(), delta["gcFloor"].clone()),
        (json!(false), json!(0))
    );
    assert_eq!(delta["rows"].as_array().unwrap().len(), 1);

    drop(engine);
    drop(phone);
    edge.shutdown().await;
    let edge = start(dir.path(), port).await;
    let full = http()
        .get(format!("{base}/registry/other-org/rows"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    // One tenant: every org path is the same room.
    assert_eq!(
        (full["seq"].clone(), full["full"].clone()),
        (json!(2), json!(true))
    );
    assert_eq!(full["rows"].as_array().unwrap().len(), 2);
    // A cursor AHEAD of the server (a wipe) forces a full answer.
    let mut late = dial(&edge, "/registry/local/ws?device=phone").await;
    late.send(Message::Text(
        json!({ "t": "hello", "cursor": 99, "device": "phone" }).to_string(),
    ))
    .await
    .unwrap();
    assert_eq!(next_json(&mut late).await["full"], true);
}

#[tokio::test]
async fn device_relay_routes_bounces_and_delivers_durable_nudges() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path(), 0).await;
    let port = edge.addr().port();
    let base = edge.url();
    let nudge = |chat: &str| {
        http()
            .post(format!("{base}/device/engine-1/nudge"))
            .bearer_auth(TOKEN)
            .json(&json!({ "chatId": chat }))
            .send()
    };

    // An unclaimed room: clients are refused and the HTTP routes 404.
    assert!(
        tokio_tungstenite::connect_async(ws_url(
            &edge,
            "/device/engine-1/ws?role=client&connId=c1"
        ))
        .await
        .is_err()
    );
    assert_eq!(nudge("chat-a").await.unwrap().status(), 404);

    // The host claims the room; a nudge queued while it is away replays on
    // its next join.
    let mut host = dial(&edge, "/device/engine-1/ws?role=host&nudgeAck=1").await;
    drop(host);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let queued = nudge("chat-a")
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(queued, json!({ "delivered": false, "queued": true }));
    host = dial(&edge, "/device/engine-1/ws?role=host&nudgeAck=1").await;
    let (header, payload) = next_device(&mut host).await;
    assert_eq!((header.s.as_str(), header.k.as_str()), ("chat-a", "nudge"));
    let receipt: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(receipt["chatId"], "chat-a");
    // ACKing the durable admission retires it: the next join replays nothing.
    send_device(
        &mut host,
        DeviceFrameHeader::new("chat-a", "nudgeAck"),
        &payload,
    )
    .await;

    // Client ⇄ host routing, with `from` stamped and `to` stripped.
    let mut client = dial(&edge, "/device/engine-1/ws?role=client&connId=c1").await;
    send_device(
        &mut client,
        DeviceFrameHeader::new("rpc", "rpc"),
        b"ping-rpc",
    )
    .await;
    let (header, payload) = next_device(&mut host).await;
    assert_eq!(
        (header.from.as_deref(), payload.as_slice()),
        (Some("c1"), &b"ping-rpc"[..])
    );
    send_device(
        &mut host,
        DeviceFrameHeader::new("rpc", "rpc").with_to("c1"),
        b"pong-rpc",
    )
    .await;
    let (header, payload) = next_device(&mut client).await;
    assert_eq!(
        (header.to, header.from, payload.as_slice()),
        (None, None, &b"pong-rpc"[..])
    );
    send_device(
        &mut host,
        DeviceFrameHeader::new("rpc", "rpc").with_to("ghost"),
        b"x",
    )
    .await;
    let (header, payload) = next_device(&mut host).await;
    assert_eq!(
        (header.k.as_str(), header.to.as_deref()),
        (RELAY_KIND, Some("ghost"))
    );
    assert_eq!(relay_code(&payload), CLIENT_GONE);

    // A live nudge reaches the connected host immediately.
    let live = nudge("chat-b")
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(live["delivered"], true);
    let (header, _) = next_device(&mut host).await;
    assert_eq!(header.s, "chat-b");
    assert_eq!(
        nudge("bad id!").await.unwrap().status(),
        400,
        "chat ids are validated"
    );

    // A client leaving tells the host; the host leaving tells the clients.
    let mut second = dial(&edge, "/device/engine-1/ws?role=client&connId=c2").await;
    drop(client);
    let (header, payload) = next_device(&mut host).await;
    assert_eq!(
        (header.k.as_str(), header.from.as_deref()),
        (RELAY_KIND, Some("c1"))
    );
    assert_eq!(relay_code(&payload), CLIENT_CLOSED);
    let status = http()
        .get(format!("{base}/device/engine-1/status"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(status["hostConnected"], true);
    drop(host);
    let (header, payload) = next_device(&mut second).await;
    assert_eq!(header.k, RELAY_KIND);
    assert_eq!(relay_code(&payload), HOST_CLOSED);
    send_device(
        &mut second,
        DeviceFrameHeader::new("rpc", "rpc"),
        b"anyone?",
    )
    .await;
    let (_, payload) = next_device(&mut second).await;
    assert_eq!(relay_code(&payload), HOST_OFFLINE);

    // Restart: the claim and the un-ACKed nudge (chat-b) persist; the ACKed
    // one (chat-a) does not come back.
    drop(second);
    edge.shutdown().await;
    let edge = start(dir.path(), port).await;
    let mut host = dial(&edge, "/device/engine-1/ws?role=host&nudgeAck=1").await;
    let (header, _) = next_device(&mut host).await;
    assert_eq!(header.s, "chat-b");
    let quiet = tokio::time::timeout(Duration::from_millis(300), host.next()).await;
    assert!(quiet.is_err(), "the ACKed nudge was retired durably");
    // A superseding host join closes the predecessor.
    let _successor = dial(&edge, "/device/engine-1/ws?role=host&nudgeAck=1").await;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), host.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), 4409);
                break;
            }
            Some(Ok(_)) => continue,
            other => panic!("expected a 4409 close, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn tool_blobs_and_the_legacy_diff_slot_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path(), 0).await;
    let base = edge.url();
    let put = http()
        .put(format!("{base}/blob/chat-1/m1%23c1"))
        .bearer_auth(TOKEN)
        .header("content-type", "text/plain; charset=utf-8")
        .body("full output")
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), 200);
    let got = http()
        .get(format!("{base}/blob/chat-1/m1%23c1"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(got.text().await.unwrap(), "full output");
    let traversal = http()
        .get(format!("{base}/blob/chat-1/a%2Fb"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(traversal.status(), 400);
    let diff = http()
        .post(format!("{base}/diff/chat-1"))
        .bearer_auth(TOKEN)
        .json(&json!({ "chatId": "chat-1", "patch": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(diff.status(), 200);
}

/// Poll `done` every 50ms for up to 5s.
async fn eventually<F: std::future::Future<Output = bool>>(what: &str, done: impl Fn() -> F) {
    for _ in 0..100 {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The engine's real host end (`zeron_rpc::HostRelay`, what `zeron headless`
/// runs): a nudge queued while it is away arrives when it joins, a live nudge
/// arrives at once, and its durable-admission ACKs retire both — a later host
/// join replays nothing.
#[tokio::test]
async fn the_engines_host_relay_receives_and_retires_nudges() {
    use std::sync::{Arc, Mutex};
    use zeron_rpc::{HostRelay, HostRelayConfig, RpcError, RpcReply, RpcService, StaticToken};

    struct NoService;
    #[async_trait::async_trait]
    impl RpcService for NoService {
        async fn handle(&self, method: &str, _: Value) -> Result<RpcReply, RpcError> {
            Err(RpcError::UnknownMethod(method.to_owned()))
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path(), 0).await;
    let base = edge.url();
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let host = || {
        let received = received.clone();
        HostRelay::spawn(
            HostRelayConfig::new(&base, "engine-2", Arc::new(StaticToken(TOKEN.into()))),
            Arc::new(NoService),
            Arc::new(move |chat| {
                received.lock().unwrap().push(chat);
                true // durably admitted → ACK
            }),
        )
    };
    let host_connected = || async {
        let status: Value = http()
            .get(format!("{base}/device/engine-2/status"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap_or_default();
        status["hostConnected"] == true
    };
    let nudge = |chat: &'static str| {
        http()
            .post(format!("{base}/device/engine-2/nudge"))
            .bearer_auth(TOKEN)
            .json(&json!({ "chatId": chat }))
            .send()
    };
    let received_now = || {
        let received = received.lock().unwrap().clone();
        async move { received }
    };

    // The host's first join claims the room; then it goes away.
    let relay = host();
    eventually("the host join", host_connected).await;
    drop(relay);
    eventually("the host to leave", || async { !host_connected().await }).await;

    // Queued while away → delivered on the next join.
    let queued: Value = nudge("chat-cold").await.unwrap().json().await.unwrap();
    assert_eq!(queued, json!({ "delivered": false, "queued": true }));
    let relay = host();
    eventually("the queued nudge", || async {
        received_now().await == ["chat-cold"]
    })
    .await;

    // Live → delivered at once.
    let live: Value = nudge("chat-warm").await.unwrap().json().await.unwrap();
    assert_eq!(live, json!({ "delivered": true, "queued": true }));
    eventually("the live nudge", || async {
        received_now().await == ["chat-cold", "chat-warm"]
    })
    .await;

    // Both were ACKed: a fresh host join (after the ACKs had time to land)
    // is handed nothing.
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(relay);
    let _relay = host();
    eventually("the host to rejoin", host_connected).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(received_now().await.len(), 2, "no nudge redelivered");
}
