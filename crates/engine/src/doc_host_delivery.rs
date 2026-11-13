//! Durable host discovery for desktop senders. The chat2 outbox already
//! recovers rows on restart; a separate receipt recovers the host wake even
//! when those rows have been acknowledged and their room is cold.

use super::*;

impl DocHost {
    /// Deliver every durable remote-wake job, at most eight chats at a time.
    /// A pass runs when the jobs change (the store's signal), when a
    /// delivery ends (its chat may still owe a wake: the 12 s yield below),
    /// or once the workspace is wired. Nothing else wakes it: with no job
    /// pending, an idle engine never reads the table. Passes stay
    /// [`SYNC_PASS_SPACING`] apart, so a job a delivery can't settle (a
    /// store read failing) retries at that pace rather than spinning.
    pub(super) fn spawn_remote_wake_delivery(&self) {
        if self.inner.config.edge.is_none() {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let kick = self.inner.remote_wake_kick.clone();
        let mut jobs = self.inner.store.watch_sync_jobs();
        self.spawn_worker(async move {
            let mut tasks = tokio::task::JoinSet::new();
            let mut active = HashSet::new();
            let mut after = String::new();
            let mut last_pass: Option<tokio::time::Instant> = None;
            loop {
                if let Some(last) = last_pass {
                    tokio::select! {
                        _ = kick.notified() => {}
                        Ok(()) = jobs.changed() => {}
                        Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                            if let Ok(chat) = result {
                                active.remove(&chat);
                            }
                        }
                    }
                    tokio::time::sleep_until(last + SYNC_PASS_SPACING).await;
                }
                last_pass = Some(tokio::time::Instant::now());
                jobs.borrow_and_update();
                while let Some(result) = tasks.try_join_next() {
                    if let Ok(chat) = result {
                        active.remove(&chat);
                    }
                }
                let Some(inner) = weak.upgrade() else { return };
                let host = DocHost { inner };
                // Page through the jobs until eight deliveries run or the
                // table ends; a full house resumes after `after` once one
                // ends, so stalled chats can't hold later ones off. A pass
                // that started mid-table goes round once from the top: a
                // chat behind the cursor (its delivery yielded) is owed too.
                let mut wrapped = after.is_empty();
                while host.workspace().is_some()
                    && host.inner.config.edge.is_some()
                    && !host.inner.edge_disconnected.load(Ordering::Acquire)
                    && active.len() < 8
                {
                    #[cfg(test)]
                    host.inner.remote_wake_scans.fetch_add(1, Ordering::Relaxed);
                    match host
                        .inner
                        .store
                        .pending_sync_jobs("remote-wake", &after, 32)
                    {
                        Ok(page) => {
                            let full = page.len() == 32;
                            let mut capped = false;
                            for chat in page {
                                if active.len() >= 8 {
                                    // Keep `after` here: the next pass goes on
                                    // past the eight, not back to them.
                                    capped = true;
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
                            if capped {
                                break;
                            }
                            if !full {
                                after.clear();
                                if wrapped {
                                    break;
                                }
                                wrapped = true;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "remote wake discovery failed");
                            // Retried on the next pass; a failed read wakes
                            // nothing, so ask for one.
                            after.clear();
                            kick.notify_one();
                            break;
                        }
                    }
                }
                drop(host);
            }
        });
    }
}

async fn deliver(weak: std::sync::Weak<DocHostInner>, chat: &str) {
    /// Out of Cloud credits (402): the edge won't start or wake the
    /// machine until credits are added. The message stays queued and the
    /// wake is retried this rarely, so adding credits delivers it.
    const AWAITING_CREDITS: std::time::Duration = std::time::Duration::from_secs(60);
    let mut backoff = std::time::Duration::from_millis(500);
    let mut awaiting_credits = false;
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
            Ok(response) if response.status().is_success() => {
                awaiting_credits = false;
                true
            }
            // Another owner's device (403) or a chat id its room refuses
            // (400): no retry can change that, so the wake is settled.
            // Unclaimed (404: not connected yet) and 5xx retry.
            Ok(response) if matches!(response.status().as_u16(), 400 | 403) => {
                tracing::warn!(%chat, device = %target, status = response.status().as_u16(), "remote host wake refused; not retrying");
                true
            }
            Ok(response) if response.status().as_u16() == 402 => {
                if !awaiting_credits {
                    tracing::warn!(%chat, device = %target, "Cloud wake refused: out of credits; retrying once credits are added");
                }
                awaiting_credits = true;
                false
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
        backoff = if awaiting_credits {
            AWAITING_CREDITS
        } else {
            (backoff * 2).min(std::time::Duration::from_secs(16))
        };
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

    /// An idle synced host reads nothing and runs no dispatch pass: both
    /// sync loops wait on events, not a clock. Ten idle minutes on a paused
    /// clock would be 6,000 passes of each with a standing 100 ms tick.
    #[tokio::test(start_paused = true)]
    async fn an_idle_host_never_wakes_its_sync_loops() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let workspace = WorkspaceHost::open(
            store.clone(),
            crate::workspace_host::WorkspaceHostConfig {
                device_id: "here".into(),
                device_name: "Here".into(),
                platform: "linux".into(),
                org_id: "org".into(),
                user_id: "user".into(),
                edge: None,
                local_only: false,
            },
        )
        .unwrap();
        workspace
            .create_chat("hosted", None, Some("here"), None, None)
            .unwrap();
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "here".into(),
                default_harness: HarnessId::Mock,
                // Nothing listens: no connection can succeed or matter.
                edge: Some(EdgeConfig::with_static_token("http://127.0.0.1:9", "test")),
            },
        );
        host.set_workspace(workspace.clone());
        let counts = |host: &DocHost| {
            (
                host.inner.scheduler_passes.load(Ordering::Relaxed),
                host.inner.remote_wake_scans.load(Ordering::Relaxed),
            )
        };
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let settled = counts(&host);
        assert!(settled.0 >= 1 && settled.1 >= 1, "both loops ran at start: {settled:?}");
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        assert_eq!(counts(&host), settled, "an idle host woke its sync loops");

        // A durable job wakes both at once; once it is handled they rest
        // again. (Reconciliation waits for registry truth, which this
        // edge-less registry never reports: no pass polls for it.)
        host.enqueue_wakeup("*").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let woken = counts(&host);
        assert!(woken.0 > settled.0, "a scheduled job wakes the dispatcher");
        assert!(woken.1 > settled.1, "and remote-wake discovery");
        assert!(store.reconciliation_progress().unwrap().is_some());
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        assert_eq!(counts(&host), woken, "pending reconciliation is no reason to poll");
        // (No chat is opened here: its join would dial through the process-
        // wide sync budget, which would keep this paused clock's far-future
        // dial time and stall every later test's dials. Opens are covered by
        // the live-transport tests in doc_host_sync_tests.)
        host.shutdown_workers().await;
    }

    /// Eight hosts that never answer hold every delivery slot; the ninth
    /// chat's wake must still go out once their deliveries yield (12 s),
    /// not wait behind the same eight forever.
    #[tokio::test(start_paused = true)]
    async fn eight_dead_hosts_cannot_starve_a_ninth_chat() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let edge = format!("http://{}", listener.local_addr().unwrap());
        let alive = Arc::new(AtomicUsize::new(0));
        let reached = alive.clone();
        let server = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                while !request.windows(4).any(|s| s == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                if String::from_utf8_lossy(&request).starts_with("POST /device/alive/nudge ") {
                    reached.fetch_add(1, Ordering::SeqCst);
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                } else {
                    held.push(stream); // a dead host: never answers
                }
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
        for n in 0..8 {
            workspace
                .create_chat(&format!("a{n}"), None, Some("dead"), None, None)
                .unwrap();
        }
        workspace.create_chat("b", None, Some("alive"), None, None).unwrap();
        // All nine owed before delivery starts: its first pass fills every
        // slot with the dead hosts (they sort first).
        for chat in (0..8).map(|n| format!("a{n}")).chain(["b".to_string()]) {
            store.schedule_sync_job(&chat, "remote-wake").unwrap();
        }
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "sender".into(),
                default_harness: HarnessId::Mock,
                edge: Some(EdgeConfig::with_static_token(&edge, "test")),
            },
        );
        host.set_workspace(workspace.clone());
        tokio::time::timeout(std::time::Duration::from_secs(120), async {
            while store.sync_job_version("b", "remote-wake").unwrap().is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the ninth chat's wake waited behind eight dead hosts");
        assert!(alive.load(Ordering::SeqCst) >= 1);
        host.shutdown_workers().await;
        server.abort();
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
                    assert!(
                        String::from_utf8_lossy(&request).starts_with("POST /device/remote/nudge ")
                    );
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
        let config = DocHostConfig {
            device_id: "sender".into(),
            default_harness: HarnessId::Mock,
            edge: Some(EdgeConfig::with_static_token(&edge, "test")),
        };
        // The send's row reached the room (acked) before its host wake goes
        // out. Done before the host starts: a pending row is real sync work,
        // and the dispatcher (which wakes on outbox writes) would admit the
        // chat and join this test edge, which answers nudges only.
        store
            .enqueue_chat_update("outgoing", "acked-row", b"command bytes")
            .unwrap();
        store
            .acknowledge_chat_update("outgoing", "acked-row")
            .unwrap();
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
