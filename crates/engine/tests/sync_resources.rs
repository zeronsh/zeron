//! Resource tests run in a child: lowering RLIMIT must not affect the test runner.
#[cfg(unix)]
#[test]
fn sync_keeps_git_available_with_256_file_descriptors() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "resource_limit_child", "--nocapture"])
        .env("ZERON_RESOURCE_TEST_CHILD", "1")
        .output()
        .unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn resource_limit_child() {
    if std::env::var_os("ZERON_RESOURCE_TEST_CHILD").is_none() {
        return;
    }
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit), 0);
        limit.rlim_cur = 256.min(limit.rlim_max);
        assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &limit), 0);
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(stress());
}

#[cfg(unix)]
async fn stress() {
    use futures::{SinkExt, StreamExt};
    use std::{sync::Arc, time::Duration};
    use zeron_engine::{DocHost, DocHostConfig, EdgeConfig};
    use zeron_proto::HarnessId;
    use zeron_sync::{
        DocsStore,
        chat_frames::{decode, encode, frame_type},
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (disconnect, _) = tokio::sync::watch::channel(0u64);
    let resets = disconnect.clone();
    let server = tokio::spawn(async move {
        let mut peers = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted.unwrap();
                    let mut reset = resets.subscribe();
                    peers.spawn(async move {
                        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
                        let mut sequence = 0u64;
                        loop {
                            let message = tokio::select! {
                                message = ws.next() => match message { Some(Ok(message)) => message, _ => break },
                                _ = reset.changed() => break,
                            };
                            if let tokio_tungstenite::tungstenite::Message::Text(text) = &message {
                                if text == "ping" { let _ = ws.send("pong".into()).await; }
                                continue;
                            }
                            let Some(frame) = decode(&message.into_data()) else { continue };
                            let reply = match frame.kind {
                                frame_type::HELLO => encode(frame_type::STATE, &serde_json::json!({"headSeq":0,"seqFloor":0,"checkpointSeq":0,"checkpointSize":0,"rowCount":0,"rowBytes":0}), &[]),
                                frame_type::ROWS_REQ => encode(frame_type::ROWS_DONE, &serde_json::json!({"headSeq":sequence}), &[]),
                                frame_type::PUSH => { sequence+=1; encode(frame_type::ACK, &serde_json::json!({"batchId":frame.header["batchId"],"seq":sequence,"dup":false}), &[]) },
                                _ => continue,
                            };
                            if ws.send(reply.into()).await.is_err() { break }
                        }
                    });
                },
                _ = peers.join_next(), if !peers.is_empty() => {},
            }
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    // A thousand durable wakeups; no eager document construction.
    for i in 0..1000 {
        store
            .schedule_sync_job(&format!("chat-{i:04}"), "wake")
            .unwrap();
    }
    let host = DocHost::new(
        store.clone(),
        DocHostConfig {
            device_id: "host".into(),
            default_harness: HarnessId::Mock,
            edge: Some(EdgeConfig::with_static_token(
                format!("http://{address}"),
                "test",
            )),
        },
    );
    let baseline = host.sync_resources();
    let mut peak = 0;
    for turn in 0..30 {
        if turn == 15 {
            disconnect.send_modify(|n| *n += 1);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let stats = host.sync_resources();
        assert!(stats["budget"]["sockets"].as_u64().unwrap() <= 24);
        assert!(stats["budget"]["http"].as_u64().unwrap() <= 8);
        assert!(stats["openDocuments"].as_u64().unwrap() <= 16, "{stats}");
        if let Some(fds) = stats["openFileDescriptors"].as_u64() {
            peak = peak.max(fds);
            assert!(fds < 224, "no reserved headroom: {stats}");
        }
        let git = tokio::process::Command::new("git")
            .arg("--version")
            .output()
            .await
            .unwrap();
        assert!(git.status.success());
    }
    assert!(
        store.sync_work_counts().unwrap().1 < 1000,
        "backlog made no progress"
    );
    tokio::time::timeout(Duration::from_secs(60), async {
        while store.sync_work_counts().unwrap() != (0, 0) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let stats = host.sync_resources();
            assert!(stats["openDocuments"].as_u64().unwrap() <= 16, "{stats}");
        }
    })
    .await
    .expect("durable backlog must eventually drain");
    host.shutdown_workers().await;
    server.abort();
    let _ = server.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while zeron_sync::budget::shared().stats().sockets != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    eprintln!(
        "baseline={baseline}; peak_fds={peak}; final={}",
        host.sync_resources()
    );
}
