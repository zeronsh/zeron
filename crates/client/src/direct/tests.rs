//! Direct mode against an in-process engine that serves a sanitized capture
//! of a real 0.2.97 desktop engine in local mode (40 chats, 23 archived, 11
//! spaces, 13 sessions with running turns, Windows paths), plus version
//! drift and a silent engine.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use zeron_proto::Chat;
use zeron_rpc::{RpcError, RpcReply, RpcService};

use super::host::{TEST_ENGINES, engine_notice};
use super::lenient::{decode_rows, parse_version, sanitize_transcript_update, unsupported_text};
use super::{DirectPhase, SshAuth, SshTarget};
use crate::events::NullListener;
use crate::{Client, ClientConfig, Credentials, lock};

fn fixture(name: &str) -> serde_json::Value {
    let text = match name {
        "EngineInfo" => include_str!("fixtures/EngineInfo.json"),
        "WatchDevices" => include_str!("fixtures/WatchDevices.json"),
        "WatchSpaces" => include_str!("fixtures/WatchSpaces.json"),
        "WatchChats" => include_str!("fixtures/WatchChats.json"),
        "WatchSessions" => include_str!("fixtures/WatchSessions.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).expect("fixture json")
}

/// Fields a newer engine might add or change.
fn drifted(method: &str, mut value: serde_json::Value) -> serde_json::Value {
    let rows = value.as_array_mut().expect("list");
    for row in rows.iter_mut() {
        row["futureField"] = serde_json::json!({"nested": [1, 2, 3]});
    }
    match method {
        // One chat loses a required field: it must be skipped, not wipe the
        // snapshot (and must not be deleted from the replica either).
        "WatchChats" => {
            let broken = rows
                .iter_mut()
                .find(|r| r["archived"] == false)
                .expect("active chat");
            broken.as_object_mut().unwrap().remove("createdAt");
        }
        // An unknown status value on one session.
        "WatchSessions" => rows[0]["status"] = serde_json::json!("hibernating"),
        _ => {}
    }
    if method == "WatchChats" {
        // A harness and a reasoning level this app has never heard of: the
        // chat must stay, just without the parts it can't read.
        let active: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r["archived"] == false && r.get("createdAt").is_some())
            .map(|(i, _)| i)
            .collect();
        rows[active[0]]["config"]["harness"] = serde_json::json!("nova-agent");
        rows[active[1]]["config"]["reasoning"] = serde_json::json!("ludicrous");
    }
    value
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Real,
    Drifted,
    /// Answers EngineInfo and every watch but WatchChats.
    SilentChats,
}

struct Engine(Mode);

#[async_trait::async_trait]
impl RpcService for Engine {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method == "EngineInfo" {
            return Ok(RpcReply::Value(fixture("EngineInfo")));
        }
        if method == "ListDrives" {
            return Ok(RpcReply::Value(serde_json::json!({
                "drives": [{ "name": "C:", "path": "C:\\" }, { "name": "D:", "path": "D:\\" }]
            })));
        }
        if method == "ListAgentAccounts" {
            // Echo the force flag in the plan label so the test sees it arrive.
            let force = params["forceUsage"].as_bool().unwrap_or(false);
            return Ok(RpcReply::Value(serde_json::json!({
                "accounts": [{
                    "id": "slot", "harness": "codex", "active": true,
                    "planLabel": if force { "Plus (forced)" } else { "Plus" },
                    "usageWindows": [{ "label": "5-hour", "usedFraction": 0.25, "resetsAt": null }]
                }],
                "warnings": []
            })));
        }
        if !matches!(
            method,
            "WatchDevices" | "WatchSpaces" | "WatchChats" | "WatchSessions"
        ) {
            return Err(RpcError::UnknownMethod(method.into()));
        }
        if self.0 == Mode::SilentChats && method == "WatchChats" {
            return Ok(RpcReply::Stream(futures::stream::pending().boxed()));
        }
        let mut item = fixture(method);
        if self.0 == Mode::Drifted {
            item = drifted(method, item);
        }
        // Snapshot, then a few quick re-emissions (running turns tick).
        let frames: Vec<serde_json::Value> = (0..4).map(|_| item.clone()).collect();
        Ok(RpcReply::Stream(
            futures::stream::iter(frames)
                .then(|f| async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    f
                })
                .chain(futures::stream::pending())
                .boxed(),
        ))
    }
}

async fn start_engine(host: &str, mode: Mode) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    crate::runtime::shared().spawn(zeron_rpc::serve_ws_listener(
        listener,
        Arc::new(Engine(mode)),
    ));
    lock(&TEST_ENGINES)
        .get_or_insert_with(Default::default)
        .insert(host.to_owned(), url);
}

fn direct_client(host: &str, dir: &std::path::Path) -> Client {
    let target = SshTarget {
        host: host.to_owned(),
        port: 22,
        user: "dev".into(),
        auth: SshAuth::Password {
            password: String::new(),
        },
        engine_port: 27654,
        host_key_fingerprint: Some("SHA256:test".into()),
    };
    let mut config = ClientConfig::new("https://edge.invalid", dir);
    config.device_id = "android-test".into();
    config.platform = "android".into();
    Client::new(config, Credentials::Direct(target), Arc::new(NullListener)).expect("client")
}

async fn wait_for(client: &Client, what: &str, ok: impl Fn(&Client) -> bool) {
    for _ in 0..200 {
        if ok(client) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}: {:?}", client.direct_status());
}

#[test]
fn real_shape_frames_decode_without_skips() {
    for method in ["WatchDevices", "WatchSpaces", "WatchChats", "WatchSessions"] {
        let value = fixture(method);
        let n = value.as_array().unwrap().len();
        let errors = match method {
            "WatchDevices" => {
                decode_rows::<zeron_proto::Device>(value, &[])
                    .unwrap()
                    .errors
            }
            "WatchSpaces" => {
                decode_rows::<zeron_proto::Space>(value, &[])
                    .unwrap()
                    .errors
            }
            "WatchChats" => decode_rows::<Chat>(value, &[]).unwrap().errors,
            _ => {
                decode_rows::<zeron_proto::Session>(value, &[])
                    .unwrap()
                    .errors
            }
        };
        assert!(errors.is_empty(), "{method}: {errors:?}");
        assert!(n > 0);
    }
}

#[test]
fn drifted_rows_are_skipped_not_fatal() {
    let decoded = decode_rows::<Chat>(drifted("WatchChats", fixture("WatchChats")), &[]).unwrap();
    assert_eq!(decoded.rows.len(), 39);
    assert_eq!(decoded.errors.len(), 1, "{:?}", decoded.errors);
    assert_eq!(decoded.repaired.len(), 2, "{:?}", decoded.repaired);
    assert!(
        decoded
            .repaired
            .iter()
            .any(|r| r.ends_with("ignored config.reasoning"))
    );
    assert!(
        decoded
            .repaired
            .iter()
            .any(|r| r.ends_with("ignored config"))
    );
    assert!(
        decoded.rows.iter().any(|c| c.config.is_none()),
        "unknown-harness chat kept"
    );
    assert_eq!(decoded.ids.len(), 40, "the broken row's id still guards it");
    // Wrapped lists and junk frames.
    let wrapped = serde_json::json!({ "chats": fixture("WatchChats"), "cursor": 3 });
    assert_eq!(decode_rows::<Chat>(wrapped, &[]).unwrap().rows.len(), 40);
    assert!(decode_rows::<Chat>(serde_json::json!("nope"), &[]).is_err());
    // Unknown session status → idle, row kept.
    let sessions = decode_rows::<zeron_proto::Session>(
        drifted("WatchSessions", fixture("WatchSessions")),
        &[("status", serde_json::json!("idle"))],
    )
    .unwrap();
    assert!(sessions.errors.is_empty(), "{:?}", sessions.errors);
    assert_eq!(sessions.rows[0].status, zeron_proto::SessionStatus::Idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn syncs_the_real_engine_shape() {
    start_engine("real.test", Mode::Real).await;
    let dir = tempfile::tempdir().unwrap();
    let client = direct_client("real.test", dir.path());
    wait_for(&client, "live", |c| {
        c.direct_status()
            .is_some_and(|s| s.phase == DirectPhase::Live)
    })
    .await;
    wait_for(&client, "rows", |c| {
        let ws = c.workspace();
        ws.archived.len() == 23 && !ws.front.recent.is_empty()
    })
    .await;
    let ws = client.workspace();
    assert_eq!(ws.devices.len(), 1);
    assert!(ws.projects.len() >= 10, "projects: {}", ws.projects.len());
    let status = client.direct_status().unwrap();
    // EngineInfo has no version; the engine's Device row supplies it.
    assert_eq!(status.engine_version.as_deref(), Some("0.2.97"));
    assert!(status.notice.is_none());
    assert!(status.last_error.is_none(), "{status:?}");
    assert!(
        status
            .streams
            .iter()
            .all(|s| s.frames > 0 && s.skipped_rows == 0)
    );
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn tolerates_version_drift() {
    start_engine("drift.test", Mode::Drifted).await;
    let dir = tempfile::tempdir().unwrap();
    let client = direct_client("drift.test", dir.path());
    wait_for(&client, "live", |c| {
        c.direct_status()
            .is_some_and(|s| s.phase == DirectPhase::Live)
    })
    .await;
    wait_for(&client, "rows", |c| c.workspace().archived.len() == 23).await;
    let status = client.direct_status().unwrap();
    let chats = status
        .streams
        .iter()
        .find(|s| s.name == "WatchChats")
        .unwrap();
    assert_eq!(
        (chats.rows, chats.skipped_rows, chats.repaired_rows),
        (39, 1, 2)
    );
    let sessions = status
        .streams
        .iter()
        .find(|s| s.name == "WatchSessions")
        .unwrap();
    assert_eq!((sessions.skipped_rows, sessions.repaired_rows), (0, 1));
    assert!(
        chats
            .error
            .as_deref()
            .is_some_and(|e| e.contains("skipped 1"))
    );
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_stream_is_reported_not_blank() {
    start_engine("silent.test", Mode::SilentChats).await;
    let dir = tempfile::tempdir().unwrap();
    let client = direct_client("silent.test", dir.path());
    wait_for(&client, "sync timeout", |c| {
        c.direct_status().is_some_and(|s| {
            s.last_error
                .as_deref()
                .is_some_and(|e| e.contains("WatchChats"))
        })
    })
    .await;
    let status = client.direct_status().unwrap();
    assert_ne!(status.phase, DirectPhase::Live);
    assert!(
        status
            .log
            .iter()
            .any(|l| l.message.contains("no WatchChats")),
        "{:?}",
        status.log
    );
    client.shutdown();
}

/// A transcript entry as a future engine might send it.
fn future_entry() -> serde_json::Value {
    serde_json::json!({
        "id": "m1",
        "role": "narrator",
        "status": "paused",
        "createdAt": 1_700_000_000_000i64,
        "deviceId": "d1",
        "mood": "curious",
        "parts": [
            { "kind": "text", "id": "p1", "text": "hello", "lang": "en" },
            { "kind": "canvas", "id": "p2", "shapes": [1, 2] },
            { "kind": "tool", "id": "p3", "call": { "kind": "browser", "url": "https://x" }, "resolved": true },
            { "kind": "tool", "id": "p4", "call": { "kind": "exec", "command": "ls" }, "subagentStatus": "queued" },
            { "kind": "reasoning", "id": "p5", "text": "hmm" }
        ]
    })
}

#[test]
fn unknown_transcript_kinds_render_as_fallbacks() {
    let mut update = serde_json::json!({
        "reset": [future_entry()],
        "contextUsage": { "tokens": "lots" },
        "cursor": 7
    });
    let repaired = sanitize_transcript_update(&mut update);
    assert!(repaired >= 4, "repaired {repaired}");
    let update: zeron_doc::TranscriptUpdate = serde_json::from_value(update).expect("readable");
    let zeron_doc::transcript_delta::TranscriptFrame::Reset { reset } = update.frame else {
        panic!("reset frame");
    };
    let entry = &reset[0];
    assert_eq!(entry.role, zeron_doc::MessageRole::Assistant);
    assert_eq!(entry.status, None);
    assert_eq!(entry.parts.len(), 5, "no part is dropped");
    use zeron_doc::MessagePart as P;
    assert!(matches!(&entry.parts[0], P::Text { text, .. } if text == "hello"));
    assert!(matches!(&entry.parts[1], P::Text { text, .. } if *text == unsupported_text("canvas")));
    assert!(matches!(
        &entry.parts[2],
        P::Tool { call: zeron_proto::ToolCall::Unknown { name, .. }, resolved: true, .. } if name == "browser"
    ));
    assert!(matches!(
        &entry.parts[3],
        P::Tool {
            call: zeron_proto::ToolCall::Exec { .. },
            subagent_status: None,
            ..
        }
    ));
    assert!(matches!(&entry.parts[4], P::Reasoning { .. }));
    assert!(update.context_usage.is_none());
}

#[test]
fn unknown_kinds_in_deltas_and_broken_entries() {
    let mut update = serde_json::json!({
        "upsert": [
            { "after": null, "entry": future_entry() },
            { "after": "m1", "entry": { "id": "m2", "parts": "not a list" } }
        ],
        "append": [],
        "remove": [],
        "count": 2
    });
    sanitize_transcript_update(&mut update);
    let update: zeron_doc::TranscriptUpdate = serde_json::from_value(update).expect("readable");
    let zeron_doc::transcript_delta::TranscriptFrame::Delta { upsert, .. } = update.frame else {
        panic!("delta frame");
    };
    assert_eq!(
        upsert.len(),
        2,
        "a broken entry becomes a placeholder, keeping the count"
    );
    assert_eq!(upsert[1].entry.id, "m2");
    // Known frames are untouched.
    let mut plain = serde_json::json!({ "reset": [] });
    assert_eq!(sanitize_transcript_update(&mut plain), 0);
}

#[test]
fn newer_minor_engines_get_a_notice_patches_do_not() {
    assert_eq!(parse_version("0.2.98"), Some((0, 2, 98)));
    assert_eq!(parse_version("v0.3.0-beta.1"), Some((0, 3, 0)));
    assert_eq!(parse_version("1"), Some((1, 0, 0)));
    assert_eq!(parse_version("nightly"), None);
    assert!(engine_notice(Some("0.2.97")).is_none());
    assert!(engine_notice(Some("0.2.140")).is_none());
    assert!(engine_notice(Some("0.3.0")).is_some_and(|n| n.contains("0.3.0")));
    assert!(engine_notice(Some("garbage")).is_none());
    assert!(engine_notice(None).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_pins_are_phone_only_and_start_empty() {
    start_engine("pins.test", Mode::Real).await;
    let dir = tempfile::tempdir().unwrap();
    let client = direct_client("pins.test", dir.path());
    wait_for(&client, "rows", |c| {
        let ws = c.workspace();
        ws.pins_ready && !ws.front.recent.is_empty()
    })
    .await;
    let ws = client.workspace();
    assert!(ws.front.pinned.is_empty(), "fresh direct link has no pins");
    let id = ws.front.recent[0].id.clone();
    assert!(!ws.session(&id).unwrap().pinned);
    client.pin_session(&id).unwrap();
    wait_for(&client, "pinned", |c| {
        c.workspace().session(&id).is_some_and(|r| r.pinned)
    })
    .await;
    let other = client
        .workspace()
        .front
        .recent
        .first()
        .map(|r| r.id.clone());
    if let Some(other) = other {
        assert!(!client.workspace().session(&other).unwrap().pinned);
    }
    client.unpin_session(&id).unwrap();
    wait_for(&client, "unpinned", |c| {
        c.workspace().session(&id).is_some_and(|r| !r.pinned)
    })
    .await;
    assert!(client.workspace().front.pinned.is_empty());
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_windows_drives_over_the_direct_link() {
    start_engine("drives.test", Mode::Real).await;
    let dir = tempfile::tempdir().unwrap();
    let client = direct_client("drives.test", dir.path());
    wait_for(&client, "live", |c| {
        c.direct_status()
            .is_some_and(|s| s.phase == DirectPhase::Live)
    })
    .await;
    let device = client.workspace().devices[0].id.clone();
    let drives = client.list_drives(&device).await.unwrap();
    let paths: Vec<&str> = drives.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["C:\\", "D:\\"]);
    // Plan usage travels the same link (ListAgentAccounts).
    let usage = client.list_agent_usage(&device, true).await.unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].plan_label.as_deref(), Some("Plus (forced)"));
    assert_eq!(usage[0].windows[0].used_fraction, 0.25);
    // Methods the engine lacks surface as Unsupported, not a dead link.
    assert!(matches!(
        client.list_folders(&device, None).await,
        Err(crate::ClientError::Unsupported(_))
    ));
    client.shutdown();
}
