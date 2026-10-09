//! Durable host discovery for desktop senders. The chat2 outbox already
//! recovers rows on restart; a separate receipt recovers the host wake even
//! when those rows have been acknowledged and their room is cold.

use super::*;

impl DocHost {
    pub(super) fn spawn_remote_wake_delivery(&self) {
        if self.inner.config.edge.is_none() {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        self.spawn_worker(async move {
            let mut tasks = tokio::task::JoinSet::new();
            let mut active = HashSet::new();
            let mut after = String::new();
            loop {
                while let Some(result) = tasks.try_join_next() {
                    if let Ok(chat) = result {
                        active.remove(&chat);
                    }
                }
                let Some(inner) = weak.upgrade() else { return };
                let host = DocHost { inner };
                if host.workspace().is_some()
                    && host.inner.config.edge.is_some()
                    && !host.inner.edge_disconnected.load(Ordering::Acquire)
                    && active.len() < 8
                {
                    match host
                        .inner
                        .store
                        .pending_sync_jobs("remote-wake", &after, 32)
                    {
                        Ok(page) => {
                            if page.is_empty() {
                                after.clear();
                            }
                            for chat in page {
                                if active.len() >= 8 {
                                    break;
                                }
                                after = chat.clone();
                                if !active.insert(chat.clone()) {
                                    continue;
                                }
                                let weak = weak.clone();
                                tasks.spawn(async move {
                                    // Yield stalled obligations so eight dead peers cannot
                                    // indefinitely block later chats on healthy hosts.
                                    let _ = tokio::time::timeout(
                                        std::time::Duration::from_secs(12),
                                        deliver(weak, &chat),
                                    )
                                    .await;
                                    chat
                                });
                            }
                        }
                        Err(err) => tracing::warn!(%err, "remote wake discovery failed"),
                    }
                }
                drop(host);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
    }
}

async fn deliver(weak: std::sync::Weak<DocHostInner>, chat: &str) {
    let mut backoff = std::time::Duration::from_millis(500);
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let host = DocHost { inner };
        let Ok(Some(version)) = host.inner.store.sync_job_version(chat, "remote-wake") else {
            return;
        };
        let Some(edge) = host.inner.config.edge.clone() else {
            return;
        };
        if host.inner.edge_disconnected.load(Ordering::Acquire) {
            return;
        }
        let Some(target) = host.remote_host_for(chat) else {
            // Registry loading can precede a restored remote row. Only
            // retire deleted/local targets after registry catch-up.
            if host
                .workspace()
                .is_some_and(|w| w.sync_status().is_some_and(|s| s.synced))
            {
                let _ = host
                    .inner
                    .store
                    .complete_sync_job(chat, "remote-wake", version);
                return;
            }
            drop(host);
            tokio::time::sleep(backoff).await;
            continue;
        };
        let rows_flushed = host.inner.store.has_pending_chat_updates(chat).ok() == Some(false);
        let sent = async {
            let bearer = edge.bearer().await?;
            let url = format!("{}/device/{target}/nudge", edge.url.trim_end_matches('/'));
            host.inner
                .http
                .post(url)
                .bearer_auth(bearer)
                .json(&serde_json::json!({ "chatId": chat }))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|err| EngineError::Other(describe_http_error(err)))
        }
        .await;
        let accepted = match sent {
            Ok(response) if response.status().is_success() => true,
            // Another owner's device (403) or a chat id its room refuses
            // (400): no retry can change that, so the wake is settled.
            // Unclaimed (404: not connected yet) and 5xx retry.
            Ok(response) if matches!(response.status().as_u16(), 400 | 403) => {
                tracing::warn!(%chat, device = %target, status = response.status().as_u16(), "remote host wake refused; not retrying");
                true
            }
            Ok(response) => {
                tracing::debug!(%chat, device = %target, status = response.status().as_u16(), "remote host wake retrying");
                false
            }
            Err(err) => {
                tracing::debug!(%chat, device = %target, %err, "remote host wake retrying");
                false
            }
        };
        if rows_flushed
            && accepted
            && host.inner.store.has_pending_chat_updates(chat).ok() == Some(false)
        {
            match host
                .inner
                .store
                .complete_sync_job(chat, "remote-wake", version)
            {
                Ok(())
                    if host.inner.store.sync_job_version(chat, "remote-wake").ok()
                        == Some(None) =>
                {
                    return;
                }
                Ok(()) => {}
                Err(err) => tracing::warn!(%err, %chat, "remote wake completion failed"),
            }
        }
        drop(host);
        let mut wake = zeron_sync::wake::subscribe();
        let mut online = zeron_sync::wake::subscribe_online();
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {},
            _ = wake.recv() => {},
            _ = online.recv() => {},
        }
        backoff = (backoff * 2).min(std::time::Duration::from_secs(16));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn remote_chat_transports_carry_the_host_on_both_paths() {
        use zeron_sync::chat_client::ChatTransport;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let edge = EdgeConfig::with_static_token(
            format!("http://{}", listener.local_addr().unwrap()),
            "test",
        )
        .with_device("sender");
        let url = edge
            .room_url_with_host("/chat2/chat/ws", Some("remote-host".into()))
            .url()
            .await
            .unwrap();
        let url = reqwest::Url::parse(&url).unwrap();
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "hostDevice" && value == "remote-host")
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|b| b == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            let line = request.lines().next().unwrap();
            assert!(line.contains("hostDevice=remote-host"), "{line}");
            assert!(line.contains("device=sender"), "{line}");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        });
        crate::chat2_host::EdgeChatTransport::new(reqwest::Client::new(), edge, "chat", "sender")
            .with_host_device(Some("remote-host".into()))
            .push("batch".into(), vec![1])
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_remote_wake_survives_row_ack_and_host_restart() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let edge = format!("http://{}", listener.local_addr().unwrap());
        let attempts = Arc::new(AtomicUsize::new(0));
        let status = Arc::new(AtomicUsize::new(500));
        let count = attempts.clone();
        let code = status.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let count = count.clone();
                let code = code.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0; 4096];
                    loop {
                        let n = stream.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..n]);
                        if request.windows(4).any(|s| s == b"\r\n\r\n") {
                            break;
                        }
                    }
                    if !String::from_utf8_lossy(&request).starts_with("POST /device/remote/nudge ")
                    {
                        return;
                    }
                    count.fetch_add(1, Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 {} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        code.load(Ordering::SeqCst)
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let workspace = WorkspaceHost::open(
            store.clone(),
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "sender".into(),
                device_name: "Sender".into(),
                platform: "linux".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap();
        workspace
            .create_chat("outgoing", None, Some("remote"), None, None)
            .unwrap();
        store
            .enqueue_chat_update("outgoing", "acked-row", b"command bytes")
            .unwrap();
        store
            .acknowledge_chat_update("outgoing", "acked-row")
            .unwrap();
        let config = DocHostConfig {
            device_id: "sender".into(),
            default_harness: HarnessId::Mock,
            edge: Some(EdgeConfig::with_static_token(&edge, "test")),
        };
        let host = DocHost::new(store.clone(), config.clone());
        host.set_workspace(workspace.clone());
        host.nudge_remote_host("outgoing");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while attempts.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("failed wake was not retried");
        assert!(
            store
                .sync_job_version("outgoing", "remote-wake")
                .unwrap()
                .is_some()
        );
        assert!(
            !store.has_pending_chat_updates("outgoing").unwrap(),
            "row delivery and wake delivery are independent"
        );
        host.shutdown_workers().await;
        workspace.flush();
        let attempts_before = attempts.load(Ordering::SeqCst);
        status.store(200, Ordering::SeqCst);
        let reopened = Arc::new(DocsStore::open(dir.path()).unwrap());
        let restarted = DocHost::new(reopened.clone(), config);
        let restored_workspace = WorkspaceHost::open(
            reopened.clone(),
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "sender".into(),
                device_name: "Sender".into(),
                platform: "linux".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap();
        restarted.set_workspace(restored_workspace);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while reopened
                .sync_job_version("outgoing", "remote-wake")
                .unwrap()
                .is_some()
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("wake was not recovered after restart");
        assert!(attempts.load(Ordering::SeqCst) > attempts_before);
        restarted.shutdown_workers().await;
        server.abort();
    }
}
