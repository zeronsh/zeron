//! Raw-value framing: the new encoder and decoder against the `Value`-based
//! ones they sit beside. The wire must not change in either direction, because
//! engines, viewports and the relay of different versions talk to each other.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::{Value, json, value::RawValue};
use tokio::sync::mpsc;

use super::*;

/// Compact items with alphabetically sorted keys, so the `Value` encoding is
/// the same bytes whether or not `serde_json` keeps insertion order.
const ITEMS: &[&str] = &[
    r#"0"#,
    r#"null"#,
    r#"true"#,
    r#""plain""#,
    r#""esc \" \\ \n \t \u0001 é 漢字 😀""#,
    r#"[1,-2,3.5,[],{}]"#,
    r#"{"after":null,"entry":{"id":"m1","parts":[{"id":"p1","text":"hi\nthere","type":"text"}],"role":"assistant"}}"#,
    r#"{"contextUsage":null,"count":3,"remove":[],"upsert":[]}"#,
    r#"{"reset":[{"createdAt":1700000000000,"id":"a","role":"user"}]}"#,
];

fn raw(text: &str) -> Box<RawValue> {
    RawValue::from_string(text.to_owned()).unwrap()
}

/// What the `Value`-based server writes for one stream item.
fn legacy_item_frame(id: u64, item: &str) -> String {
    serde_json::to_string(&ServerFrame {
        id,
        item: Some(serde_json::from_str(item).unwrap()),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn item_frame_is_byte_identical_to_the_server_frame_encoding() {
    for id in [0, 1, 7, 4_294_967_296, u64::MAX] {
        for item in ITEMS {
            assert_eq!(
                item_frame(id, &raw(item)),
                legacy_item_frame(id, item),
                "id {id}, item {item}"
            );
        }
    }
}

#[test]
fn item_frame_keeps_one_line_for_pretty_printed_bytes() {
    let pretty = "{\n  \"a\": [1,\n 2],\r\n  \"b\": \"x\\ny\"\n}";
    let frame = item_frame(3, &raw(pretty));
    assert!(!frame.contains(['\n', '\r']), "{frame:?}");
    assert_eq!(frame, r#"{"id":3,"item":{"a":[1,2],"b":"x\ny"}}"#);
}

#[test]
fn envelope_reads_every_frame_shape_like_server_frame() {
    let lines = [
        r#"{"id":1,"ok":{"a":1}}"#,
        r#"{"id":2,"ok":null}"#,
        r#"{"id":3,"ok":[]}"#,
        r#"{"id":4}"#,
        r#"{"id":5,"err":"boom"}"#,
        r#"{"id":6,"err":null}"#,
        r#"{"id":7,"item":{"x":[1,2]}}"#,
        r#"{"id":8,"item":null}"#,
        r#"{"id":9,"done":true}"#,
        r#"{"id":10,"done":false,"item":0}"#,
        // A newer peer may add fields; both readers ignore them.
        r#"{"id":11,"item":"s","later":{"k":1},"done":true}"#,
        // Whitespace the compact encoder never writes.
        r#"{ "id" : 12 , "item" : { "k" : [ 1 , 2 ] } }"#,
    ];
    for line in lines {
        let old: ServerFrame = serde_json::from_str(line).unwrap();
        let new: ServerFrameRef = serde_json::from_str(line).unwrap();
        assert_eq!(new.id, old.id, "{line}");
        let parse = |raw: Option<&RawValue>| raw.map(|r| serde_json::from_str::<Value>(r.get()));
        assert_eq!(parse(new.ok).map(Result::unwrap), old.ok, "ok in {line}");
        assert_eq!(new.err, old.err, "err in {line}");
        assert_eq!(
            parse(new.item).map(Result::unwrap),
            old.item,
            "item in {line}"
        );
        assert_eq!(new.done, old.done, "done in {line}");
    }
    assert!(serde_json::from_str::<ServerFrameRef>(r#"{"ok":1}"#).is_err());
    assert!(serde_json::from_str::<ServerFrameRef>(r#"{"id":1,"item":"#).is_err());
}

/// Serves `items` as a stream of parsed values (what every server did before
/// raw framing) or as raw bytes (the new server path).
struct ItemsService {
    raw: bool,
    items: &'static [&'static str],
}

#[async_trait]
impl RpcService for ItemsService {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            "Items" if self.raw => Ok(RpcReply::RawStream(
                futures::stream::iter(self.items.iter().map(|item| raw(item))).boxed(),
            )),
            "Items" => Ok(RpcReply::Stream(
                futures::stream::iter(
                    self.items
                        .iter()
                        .map(|item| serde_json::from_str(item).unwrap()),
                )
                .boxed(),
            )),
            "Echo" => Ok(RpcReply::Value(params)),
            "Boom" => Err(RpcError::Failed("boom".into())),
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

/// Every line a server writes for one request, through the real dispatch loop.
async fn server_lines(service: ItemsService, request: &str) -> Vec<String> {
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<String>(256);
    tokio::spawn(serve_connection(Arc::new(service), out_tx, in_rx));
    in_tx.send(request.to_owned()).await.unwrap();
    let mut lines = Vec::new();
    while let Some(line) = out_rx.recv().await {
        let done = line.contains("\"done\":true") || line.contains("\"err\"");
        lines.push(line);
        if done {
            break;
        }
    }
    lines
}

#[tokio::test]
async fn raw_stream_writes_the_same_lines_as_a_value_stream() {
    let request = r#"{"id":5,"method":"Items"}"#;
    let old = server_lines(
        ItemsService {
            raw: false,
            items: ITEMS,
        },
        request,
    )
    .await;
    let new = server_lines(
        ItemsService {
            raw: true,
            items: ITEMS,
        },
        request,
    )
    .await;
    assert_eq!(new, old);
    assert_eq!(new.len(), ITEMS.len() + 1);
    assert_eq!(new.last().unwrap(), r#"{"id":5,"done":true}"#);
}

#[tokio::test]
async fn old_client_decodes_a_new_servers_raw_stream() {
    let lines = server_lines(
        ItemsService {
            raw: true,
            items: ITEMS,
        },
        r#"{"id":9,"method":"Items"}"#,
    )
    .await;
    // The pre-raw reader: the whole frame through `ServerFrame`.
    let frames: Vec<ServerFrame> = lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let (done, items) = frames.split_last().unwrap();
    assert!(done.done && done.item.is_none() && done.ok.is_none() && done.err.is_none());
    for (frame, expected) in items.iter().zip(ITEMS) {
        assert_eq!(frame.id, 9);
        let expected: Value = serde_json::from_str(expected).unwrap();
        // `"item": null` reads as absent for an old client, raw or not.
        assert_eq!(frame.item, (!expected.is_null()).then_some(expected));
    }
}

#[tokio::test]
async fn new_client_reads_an_old_servers_value_stream_as_raw_bytes() {
    let client = memory_client(Arc::new(ItemsService {
        raw: false,
        items: ITEMS,
    }));
    // `null` items are invisible on the wire to both readers.
    let expected: Vec<&str> = ITEMS.iter().copied().filter(|i| *i != "null").collect();

    let mut scoped = client
        .subscribe_scoped_raw("Items", Value::Null)
        .await
        .unwrap();
    let mut seen = Vec::new();
    while let Some(item) = scoped.recv().await {
        seen.push(item.get().to_owned());
    }
    assert_eq!(seen, expected);

    let mut checked = client
        .subscribe_checked_raw("Items", Value::Null)
        .await
        .unwrap();
    let mut seen = Vec::new();
    while let Some(item) = checked.recv().await {
        seen.push(item.get().to_owned());
    }
    assert_eq!(seen, expected);

    let mut legacy_raw = client.subscribe_raw("Items", Value::Null).await.unwrap();
    let mut seen = Vec::new();
    while let Some(item) = legacy_raw.recv().await {
        seen.push(item.get().to_owned());
    }
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn value_consumers_read_a_raw_stream_unchanged() {
    let client = memory_client(Arc::new(ItemsService {
        raw: true,
        items: ITEMS,
    }));
    let expected: Vec<Value> = ITEMS
        .iter()
        .map(|item| serde_json::from_str(item).unwrap())
        .filter(|value: &Value| !value.is_null())
        .collect();

    let mut legacy = client.subscribe("Items", Value::Null).await.unwrap();
    let mut seen = Vec::new();
    while let Some(item) = legacy.recv().await {
        seen.push(item);
    }
    assert_eq!(seen, expected);

    let mut scoped = client.subscribe_scoped("Items", Value::Null).await.unwrap();
    let mut seen = Vec::new();
    while let Some(item) = scoped.recv().await {
        seen.push(item);
    }
    assert_eq!(seen, expected);

    let mut checked = client
        .subscribe_checked("Items", Value::Null)
        .await
        .unwrap();
    let mut seen = Vec::new();
    while let Some(item) = checked.recv().await {
        seen.push(item);
    }
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn raw_client_still_gets_unary_replies_errors_and_unknown_methods() {
    let client = memory_client(Arc::new(ItemsService {
        raw: true,
        items: ITEMS,
    }));
    assert_eq!(
        client.call("Echo", json!({"k": [1, null]})).await.unwrap(),
        json!({"k": [1, null]})
    );
    assert_eq!(client.call("Echo", Value::Null).await.unwrap(), Value::Null);
    assert!(matches!(
        client.call("Boom", Value::Null).await.unwrap_err(),
        RpcError::Failed(m) if m == "boom"
    ));
    let Err(error) = client.subscribe_checked_raw("Nope", Value::Null).await else {
        panic!("an unknown stream must be rejected");
    };
    assert!(matches!(error, RpcError::UnknownMethod(m) if m == "Nope"));
}

/// A stream that never yields and reports when the server drops it.
struct SilentRaw(Mutex<Option<tokio::sync::oneshot::Sender<()>>>);

struct Dropped(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for Dropped {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[async_trait]
impl RpcService for SilentRaw {
    async fn handle(&self, _method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        let guard = Dropped(self.0.lock().unwrap().take());
        Ok(RpcReply::RawStream(
            futures::stream::unfold(guard, |_guard| async move {
                std::future::pending::<Option<(Box<RawValue>, Dropped)>>().await
            })
            .boxed(),
        ))
    }
}

#[tokio::test]
async fn dropping_a_raw_subscription_cancels_the_server_stream() {
    let (tx, dropped) = tokio::sync::oneshot::channel();
    let service = Arc::new(SilentRaw(Mutex::new(Some(tx))));
    let client = memory_client(service.clone());
    let subscription = client
        .subscribe_scoped_raw("Silent", Value::Null)
        .await
        .unwrap();
    while service.0.lock().unwrap().is_some() {
        tokio::task::yield_now().await;
    }
    drop(subscription);
    tokio::time::timeout(std::time::Duration::from_secs(1), dropped)
        .await
        .expect("server stream cancelled")
        .expect("drop signal");
}

#[tokio::test]
async fn into_value_stream_parses_raw_items() {
    let reply = RpcReply::RawStream(futures::stream::iter([raw(r#"{"a":[1]}"#), raw("2")]).boxed());
    let values: Vec<Value> = reply.into_value_stream().ok().unwrap().collect().await;
    assert_eq!(values, vec![json!({"a": [1]}), json!(2)]);
    assert!(RpcReply::Value(json!(1)).into_value_stream().is_err());
}
