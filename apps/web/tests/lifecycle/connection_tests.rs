use crate::engine_connection::{EngineHandle, EngineMode};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use tokio::{
    task::LocalSet,
    time::{Duration, timeout},
};
use zeron_proto::{EngineInfo, WorkspaceScope};
use zeron_rpc::device_frame::{DeviceFrameHeader, RPC_KIND, encode_device_frame};
use zeron_rpc::{ClientFrame, RpcClient, RpcError, methods};

use crate::browser_connection::{
    ConnectionEpochs, Inbox, MAX_BUFFERED, MAX_OUTBOUND_FRAME, OutboundFramePolicy, Signal,
    SocketState, outbound_frame_policy, pump, signal,
};
use crate::browser_session::{
    DeviceDto, LifecycleCoordinator, MAX_AUTOMATIC_RECONNECTS, NO_ONLINE_DEVICES_MESSAGE,
    OnlineDeviceCandidates, ReconnectPlan, ReconnectState, browser_connection_failure_message,
    consume_reconnect, next_reconnect_attempt, reconnect_plan,
};
fn transport() -> (RpcClient, mpsc::Receiver<String>, mpsc::Sender<String>) {
    let (out, requests) = mpsc::channel(4);
    let (responses, inbound) = mpsc::channel(4);
    (RpcClient::new(out, inbound), requests, responses)
}

async fn request(requests: &mut mpsc::Receiver<String>, method: &str) -> ClientFrame {
    let frame: ClientFrame = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
    assert_eq!(frame.method.as_deref(), Some(method));
    assert!(!frame.cancel);
    frame
}

async fn reply(responses: &mpsc::Sender<String>, id: u64, value: Value) {
    responses
        .send(json!({"id": id, "ok": value}).to_string())
        .await
        .unwrap();
}

fn info() -> EngineInfo {
    EngineInfo {
        device_id: "test-device".into(),
        workspace_scope: WorkspaceScope::Local,
        capabilities: Vec::new(),
    }
}

#[tokio::test]
async fn connected_handle_requires_identity_and_readiness_and_only_closes_viewport() {
    let (client, mut requests, responses) = transport();
    let attach = EngineHandle::from_connected_client(client, "ws://test/api/rpc".into());
    let peer = async {
        let identity = request(&mut requests, methods::ENGINE_INFO).await;
        reply(
            &responses,
            identity.id,
            serde_json::to_value(info()).unwrap(),
        )
        .await;
        let ready = request(&mut requests, methods::ENGINE_READY).await;
        reply(&responses, ready.id, json!({"ready": true})).await;
    };
    let (handle, ()) = tokio::join!(attach, peer);
    let handle = handle.unwrap();
    assert_eq!(handle.engine_info().device_id, "test-device");
    assert_eq!(handle.engine_info().workspace_scope, WorkspaceScope::Local);
    assert_eq!(
        handle.mode(),
        EngineMode::Remote {
            url: "ws://test/api/rpc".into()
        }
    );
    assert!(
        requests.try_recv().is_err(),
        "attachment must not start watches or mutations"
    );
    let clone = handle.clone();
    handle.shutdown().await;
    // The server receives no StopEngine (or other command), only EOF.
    assert!(requests.recv().await.is_none());
    responses.closed().await;
    assert!(matches!(
        clone.client().call("AfterClose", json!({})).await,
        Err(RpcError::Closed)
    ));
}

#[tokio::test]
async fn connected_handle_rejects_not_ready_without_starting_watches() {
    let (client, mut requests, responses) = transport();
    let attach = EngineHandle::from_connected_client(client, "ws://test/api/rpc".into());
    let peer = async {
        let identity = request(&mut requests, methods::ENGINE_INFO).await;
        reply(
            &responses,
            identity.id,
            serde_json::to_value(info()).unwrap(),
        )
        .await;
        let ready = request(&mut requests, methods::ENGINE_READY).await;
        reply(&responses, ready.id, json!({"ready": false})).await;
    };
    let (result, ()) = tokio::join!(attach, peer);
    assert!(matches!(result, Err(RpcError::Failed(_))));
    assert!(requests.recv().await.is_none());
}

#[tokio::test]
async fn dropping_attachment_during_readiness_releases_transport() {
    let (client, mut requests, responses) = transport();
    let mut attach = Box::pin(EngineHandle::from_connected_client(
        client,
        "ws://test/api/rpc".into(),
    ));
    let peer = async {
        let identity = request(&mut requests, methods::ENGINE_INFO).await;
        reply(
            &responses,
            identity.id,
            serde_json::to_value(info()).unwrap(),
        )
        .await;
        request(&mut requests, methods::ENGINE_READY).await;
    };
    tokio::select! {
        _ = &mut attach => panic!("attachment completed before readiness"),
        () = peer => {},
    }
    drop(attach);
    // Cancellation may already be queued; no other RPC may escape.
    while let Some(text) = requests.recv().await {
        let frame: ClientFrame = serde_json::from_str(&text).unwrap();
        assert!(frame.cancel);
    }
    responses.closed().await;
}

#[test]
fn auth_and_socket_epochs_reject_stale_attachment_results() {
    let mut epochs = ConnectionEpochs::default();
    let signed_in = epochs.begin_auth();
    assert!(epochs.is_current(signed_in));

    let first_socket = epochs.begin_socket();
    assert!(epochs.is_current(first_socket));
    assert!(!epochs.is_current(signed_in));

    let reconnect = epochs.begin_socket();
    assert!(epochs.is_current(reconnect));
    assert!(!epochs.is_current(first_socket));

    let signed_out = epochs.begin_auth();
    assert!(epochs.is_current(signed_out));
    assert!(!epochs.is_current(reconnect));
}

#[test]
fn request_epoch_cancels_outstanding_requests_and_rejects_late_starts() {
    let mut coordinator = LifecycleCoordinator::default();
    let (first_epoch, cancelled) = coordinator.begin_epoch();
    assert!(cancelled.is_empty());
    let first = coordinator.begin_request(first_epoch).unwrap();
    let second = coordinator.begin_request(first_epoch).unwrap();

    let (next_epoch, cancelled) = coordinator.begin_epoch();
    assert_eq!(cancelled, vec![first, second]);
    assert!(coordinator.begin_request(first_epoch).is_none());
    let current = coordinator.begin_request(next_epoch).unwrap();
    coordinator.finish_request(current);
    let (_, cancelled) = coordinator.begin_epoch();
    assert!(cancelled.is_empty());
}

#[test]
fn activity_requires_real_input_and_is_throttled() {
    let mut coordinator = LifecycleCoordinator::default();
    const INTERVAL_MS: f64 = 60_000.0;

    // Calling this represents a keyboard or pointer event; idle time never does.
    assert!(coordinator.should_report_activity(1_000.0, INTERVAL_MS));
    assert!(!coordinator.should_report_activity(1_001.0, INTERVAL_MS));
    assert!(!coordinator.should_report_activity(60_999.0, INTERVAL_MS));
    assert!(coordinator.should_report_activity(61_000.0, INTERVAL_MS));
}

#[test]
fn browser_fallback_advances_after_first_candidate_fails() {
    let devices = vec![
        DeviceDto {
            id: "second".into(),
            online: true,
        },
        DeviceDto {
            id: "first".into(),
            online: true,
        },
        DeviceDto {
            id: "offline".into(),
            online: false,
        },
    ];
    let mut candidates = OnlineDeviceCandidates::from_devices(&devices);
    let mut tried = Vec::new();

    while let Some(device_id) = candidates.next() {
        tried.push(device_id.to_owned());
        if device_id == "second" {
            break;
        }
    }

    assert_eq!(tried, ["first", "second"]);
}

#[test]
fn browser_fallback_exhausts_online_candidates() {
    let devices = vec![
        DeviceDto {
            id: "second".into(),
            online: true,
        },
        DeviceDto {
            id: "first".into(),
            online: true,
        },
    ];
    let mut candidates = OnlineDeviceCandidates::from_devices(&devices);

    assert_eq!(candidates.next(), Some("first"));
    assert_eq!(candidates.next(), Some("second"));
    assert_eq!(candidates.next(), None);
}

#[test]
fn browser_selection_has_no_candidate_when_all_devices_are_offline() {
    let devices = vec![
        DeviceDto {
            id: "offline".into(),
            online: false,
        },
        DeviceDto {
            id: "".into(),
            online: true,
        },
    ];

    let candidates = OnlineDeviceCandidates::from_devices(&devices);
    assert!(candidates.is_empty());
    assert_eq!(
        NO_ONLINE_DEVICES_MESSAGE,
        "No online devices are available for this account. Open Zeron on a device and try again.",
    );
}

#[test]
fn stale_browser_retry_cannot_advance_an_old_fallback() {
    let mut epochs = ConnectionEpochs::default();
    let auth = epochs.begin_auth();
    let first = epochs.begin_socket();
    assert!(!epochs.is_current(auth));
    assert!(epochs.is_current(first));

    let retry_auth = epochs.begin_auth();
    assert!(epochs.is_current(retry_auth));
    assert!(!epochs.is_current(first));

    let retry = epochs.begin_socket();
    assert!(!epochs.is_current(first));
    assert!(epochs.is_current(retry));
}

#[test]
fn browser_failure_and_disconnect_copy_is_retryable_and_does_not_replay() {
    assert_eq!(
        browser_connection_failure_message("RPC socket closed during handshake"),
        "Could not connect to your online device: RPC socket closed during handshake. Try again.",
    );
}

#[test]
fn timer_consumption_survives_hide_before_dispatch_and_reschedules_on_return() {
    let mut state = ReconnectState {
        pending: true,
        scheduled: false,
    };
    assert_eq!(
        reconnect_plan(true, true, state.scheduled),
        ReconnectPlan::Schedule
    );

    state.scheduled = true;
    let (state, start) = consume_reconnect(state, true, false);
    assert_eq!(
        state,
        ReconnectState {
            pending: true,
            scheduled: false,
        }
    );
    assert!(!start);
    assert_eq!(
        reconnect_plan(state.pending, true, state.scheduled),
        ReconnectPlan::Schedule
    );

    let (state, start) = consume_reconnect(state, true, true);
    assert_eq!(
        state,
        ReconnectState {
            pending: false,
            scheduled: false,
        }
    );
    assert!(start);

    let stale = ReconnectState {
        pending: true,
        scheduled: true,
    };
    assert_eq!(consume_reconnect(stale, false, true), (stale, false));
}

#[test]
fn immediate_disconnects_are_limited_until_a_connection_stays_stable() {
    let mut attempts = 0;
    for expected in 1..=MAX_AUTOMATIC_RECONNECTS {
        attempts = next_reconnect_attempt(attempts).unwrap();
        assert_eq!(attempts, expected);
    }
    assert_eq!(next_reconnect_attempt(attempts), None);
}

struct TestSocket;

impl crate::browser_connection::SocketSink for TestSocket {
    async fn send(&self, _: &str) -> Result<(), ()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pump_keeps_a_popped_frame_across_an_ordinary_socket_notification() {
    LocalSet::new()
        .run_until(async {
            use crate::browser_connection::{Inbox, Signal, SocketState, pump, signal};
            use std::{cell::RefCell, rc::Rc};

            let inbox = Rc::new(RefCell::new(Inbox::default()));
            assert!(inbox.borrow_mut().push("payload".into()));
            let (signal_tx, signal_rx) = tokio::sync::watch::channel(Signal {
                state: SocketState::Open,
                sequence: 0,
            });
            let (_outbound_tx, outbound_rx) = mpsc::channel(1);
            let (inbound_tx, mut inbound_rx) = mpsc::channel(1);
            inbound_tx.send("occupied".into()).await.unwrap();

            let task = tokio::task::spawn_local(pump(
                TestSocket,
                inbox,
                signal_rx,
                outbound_rx,
                inbound_tx,
            ));
            tokio::task::yield_now().await;
            signal(&signal_tx, SocketState::Open);
            tokio::task::yield_now().await;

            assert_eq!(inbound_rx.recv().await.as_deref(), Some("occupied"));
            assert_eq!(
                timeout(Duration::from_millis(100), inbound_rx.recv())
                    .await
                    .expect("ordinary open notification must not discard a queued frame")
                    .as_deref(),
                Some("payload")
            );
            signal(&signal_tx, SocketState::Closed);
            task.await.unwrap();
        })
        .await;
}

struct BackpressuredSocket {
    sent: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    release: std::rc::Rc<Notify>,
}

impl crate::browser_connection::SocketSink for BackpressuredSocket {
    async fn send(&self, text: &str) -> Result<(), ()> {
        self.release.notified().await;
        self.sent.borrow_mut().push(text.to_string());
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn large_upload_frame_is_not_dropped_by_socket_buffer_cap() {
    LocalSet::new()
        .run_until(async {
            let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let release = std::rc::Rc::new(Notify::new());
            let (_signal_tx, signal_rx) = tokio::sync::watch::channel(Signal {
                state: SocketState::Open,
                sequence: 0,
            });
            let (outbound_tx, outbound_rx) = mpsc::channel(1);
            let (inbound_tx, mut inbound_rx) = mpsc::channel(1);
            let upload_chunk = "x".repeat((200usize * 1024).div_ceil(3) * 4);
            outbound_tx.send(upload_chunk.clone()).await.unwrap();
            drop(outbound_tx);

            let task = tokio::task::spawn_local(pump(
                BackpressuredSocket {
                    sent: sent.clone(),
                    release: release.clone(),
                },
                std::rc::Rc::new(std::cell::RefCell::new(Inbox::default())),
                signal_rx,
                outbound_rx,
                inbound_tx,
            ));
            tokio::task::yield_now().await;
            assert!(sent.borrow().is_empty());
            release.notify_one();
            task.await.unwrap();
            assert_eq!(inbound_rx.recv().await, None);
            assert_eq!(sent.borrow().as_slice(), [upload_chunk]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn consecutive_upload_chunks_wait_for_capacity_in_order() {
    LocalSet::new()
        .run_until(async {
            let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let release = std::rc::Rc::new(Notify::new());
            let (_signal_tx, signal_rx) = tokio::sync::watch::channel(Signal {
                state: SocketState::Open,
                sequence: 0,
            });
            let (outbound_tx, outbound_rx) = mpsc::channel(3);
            let (inbound_tx, _inbound_rx) = mpsc::channel(1);
            let total_b64 = (1usize * 1024 * 1024).div_ceil(3) * 4;
            let chunks = vec![
                "a".repeat(680_000),
                "b".repeat(680_000),
                "c".repeat(total_b64 - 1_360_000),
            ];
            for chunk in &chunks {
                outbound_tx.send(chunk.clone()).await.unwrap();
            }
            drop(outbound_tx);

            let task = tokio::task::spawn_local(pump(
                BackpressuredSocket {
                    sent: sent.clone(),
                    release: release.clone(),
                },
                std::rc::Rc::new(std::cell::RefCell::new(Inbox::default())),
                signal_rx,
                outbound_rx,
                inbound_tx,
            ));
            for (index, chunk) in chunks.iter().enumerate() {
                tokio::task::yield_now().await;
                assert_eq!(sent.borrow().len(), index);
                release.notify_one();
                timeout(Duration::from_millis(100), async {
                    while sent.borrow().len() <= index {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("backpressure release must advance one chunk");
                assert_eq!(sent.borrow()[index], *chunk);
            }
            task.await.unwrap();
            assert_eq!(sent.borrow().len(), chunks.len());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn disconnect_cancels_pending_chunk_without_replaying_sent_mutations() {
    LocalSet::new()
        .run_until(async {
            let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let release = std::rc::Rc::new(Notify::new());
            let (signal_tx, signal_rx) = tokio::sync::watch::channel(Signal {
                state: SocketState::Open,
                sequence: 0,
            });
            let (outbound_tx, outbound_rx) = mpsc::channel(2);
            let (inbound_tx, mut inbound_rx) = mpsc::channel(1);
            outbound_tx.send("mutation-1".into()).await.unwrap();
            outbound_tx.send("mutation-2".into()).await.unwrap();

            let task = tokio::task::spawn_local(pump(
                BackpressuredSocket {
                    sent: sent.clone(),
                    release: release.clone(),
                },
                std::rc::Rc::new(std::cell::RefCell::new(Inbox::default())),
                signal_rx,
                outbound_rx,
                inbound_tx,
            ));
            tokio::task::yield_now().await;
            release.notify_one();
            timeout(Duration::from_millis(100), async {
                while sent.borrow().len() < 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("first mutation must be sent once");
            tokio::task::yield_now().await;
            signal(&signal_tx, SocketState::Closed);
            task.await.unwrap();
            release.notify_one();
            tokio::task::yield_now().await;
            assert_eq!(sent.borrow().as_slice(), ["mutation-1"]);
            assert_eq!(inbound_rx.recv().await, None);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_send_still_delivers_inbound_across_repeated_open_signals() {
    LocalSet::new()
        .run_until(async {
            let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let release = std::rc::Rc::new(Notify::new());
            let (signal_tx, signal_rx) = tokio::sync::watch::channel(Signal {
                state: SocketState::Open,
                sequence: 0,
            });
            let (outbound_tx, outbound_rx) = mpsc::channel(1);
            let (inbound_tx, mut inbound_rx) = mpsc::channel(1);
            let inbox = std::rc::Rc::new(std::cell::RefCell::new(Inbox::default()));
            outbound_tx.send("blocked-upload".into()).await.unwrap();

            let task = tokio::task::spawn_local(pump(
                BackpressuredSocket {
                    sent: sent.clone(),
                    release: release.clone(),
                },
                inbox.clone(),
                signal_rx,
                outbound_rx,
                inbound_tx,
            ));
            tokio::task::yield_now().await;
            assert!(sent.borrow().is_empty());
            assert!(inbox.borrow_mut().push("response".into()));
            for _ in 0..8 {
                signal(&signal_tx, SocketState::Open);
                tokio::task::yield_now().await;
            }

            assert_eq!(
                timeout(Duration::from_millis(100), inbound_rx.recv())
                    .await
                    .expect("inbound delivery must not wait for outbound drain")
                    .as_deref(),
                Some("response")
            );
            assert!(sent.borrow().is_empty(), "outbound send is still blocked");

            release.notify_one();
            timeout(Duration::from_millis(100), async {
                while sent.borrow().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the original pending send must complete once released");
            signal(&signal_tx, SocketState::Closed);
            task.await.unwrap();
            assert_eq!(sent.borrow().as_slice(), ["blocked-upload"]);
        })
        .await;
}

#[test]
fn browser_policy_allows_a_200k_upload_frame_over_the_high_water_mark() {
    let payload = vec![0; (200usize * 1024).div_ceil(3) * 4];
    let frame = encode_device_frame(&DeviceFrameHeader::new(RPC_KIND, RPC_KIND), &payload).unwrap();
    assert!(frame.len() > MAX_BUFFERED as usize);
    assert_eq!(
        outbound_frame_policy(0, frame.len()),
        OutboundFramePolicy::Send
    );
}

#[test]
fn browser_policy_accepts_encoded_one_mib_boundary_and_rejects_oversize() {
    let header = DeviceFrameHeader::new(RPC_KIND, RPC_KIND);
    let overhead = encode_device_frame(&header, &[]).unwrap().len();
    let exact = encode_device_frame(&header, &vec![0; MAX_OUTBOUND_FRAME - overhead]).unwrap();
    assert_eq!(exact.len(), MAX_OUTBOUND_FRAME);
    assert_eq!(
        outbound_frame_policy(0, exact.len()),
        OutboundFramePolicy::Send
    );
    assert_eq!(
        outbound_frame_policy(0, exact.len() + 1),
        OutboundFramePolicy::Reject
    );
}

#[test]
fn browser_policy_waits_for_buffer_drain_between_consecutive_chunks() {
    let chunk = encode_device_frame(
        &DeviceFrameHeader::new(RPC_KIND, RPC_KIND),
        &vec![0; 680_000],
    )
    .unwrap()
    .len();
    assert_eq!(outbound_frame_policy(0, chunk), OutboundFramePolicy::Send);
    assert_eq!(
        outbound_frame_policy(chunk as u32, chunk),
        OutboundFramePolicy::Wait
    );
    assert_eq!(
        outbound_frame_policy(MAX_BUFFERED, chunk),
        OutboundFramePolicy::Send
    );
}
