//! The room outbox and host discovery are separate delivery obligations.
//! Persist the wake until the edge accepts it AND all local rows are ACKed.
//! Restart recovery opens these rooms without depending on viewport/preloads.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::client::ClientInner;

pub(crate) const JOB: &str = "viewer-delivery";
const MAX_ACTIVE: usize = 8;

pub(crate) fn start(inner: &Arc<ClientInner>, wake: Arc<Notify>, cancel: CancellationToken) {
    let weak = Arc::downgrade(inner);
    crate::runtime::shared().spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        let mut active = HashSet::new();
        let mut after = String::new();
        loop {
            let notified = wake.notified();
            while let Some(result) = tasks.try_join_next() {
                if let Ok(chat) = result {
                    active.remove(&chat);
                }
            }
            let Some(inner) = weak.upgrade() else { return };
            let Some(live) = inner.live() else { return };
            // Preserve the scan cursor while full: wrapping here would let
            // the same first page reclaim every released slot.
            if active.len() < MAX_ACTIVE {
                match live.store.pending_viewer_deliveries(&after, 64) {
                    Ok(page) => {
                        if page.is_empty() {
                            after.clear();
                        }
                        for chat in page {
                            if active.len() >= MAX_ACTIVE {
                                break;
                            }
                            after = chat.clone();
                            if inner.workspace.chat(&chat).is_none() || !active.insert(chat.clone()) {
                                continue;
                            }
                            let inner = Arc::downgrade(&inner);
                            let cancel = cancel.clone();
                            tasks.spawn(async move {
                                tokio::select! {
                                    _ = cancel.cancelled() => {},
                                    // Yield stalled receipts to later destinations.
                                    _ = tokio::time::timeout(Duration::from_secs(12), deliver(inner, &chat)) => {},
                                }
                                chat
                            });
                        }
                    }
                    Err(err) => tracing::warn!(%err, "delivery discovery failed"),
                }
            }
            drop(inner);
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = notified => {},
                _ = tokio::time::sleep(Duration::from_millis(500)) => {},
            }
        }
    });
}

async fn deliver(weak: std::sync::Weak<ClientInner>, chat: &str) {
    // Hold the session (not the account) while delivering: a detached room
    // must remain alive through its outgoing ACK, including queue-only writes.
    let _session = {
        let Some(inner) = weak.upgrade() else { return };
        let Ok(session) = inner.resume_delivery(chat) else {
            return;
        };
        session
    };
    let mut backoff = super::JOIN_RETRY_BASE;
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let Some(live) = inner.live() else { return };
        let Some(row) = inner.workspace.chat(chat) else {
            return;
        };
        let version = match live.store.sync_job_version(chat, JOB) {
            Ok(version) => version,
            Err(err) => {
                tracing::warn!(%err, %chat, "delivery receipt load failed");
                None
            }
        };
        let rows_flushed = live.store.has_pending_chat_updates(chat).ok() == Some(false);
        // No registry wait: hosts durably admit a wake before CreateChat
        // arrives and default missing rows to chat2 (the born-chat race).
        let result = async {
            let token = inner.tokens.bearer().await?;
            let response = crate::auth::http()
                .post(super::urls::nudge(&live.edge, &row.device_id))
                .bearer_auth(token)
                .json(&serde_json::json!({ "chatId": chat }))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|err| crate::ClientError::Network(err.to_string()))?;
            // Another owner's device (403) or a chat id its room refuses
            // (400): no retry can change that, so the wake is settled. The
            // receipt still waits for the outgoing rows. Unclaimed (404:
            // the host has not connected yet) and 5xx retry.
            if matches!(response.status().as_u16(), 400 | 403) {
                tracing::warn!(%chat, status = response.status().as_u16(), "host wake refused; not retrying");
                return Ok(());
            }
            if !response.status().is_success() {
                return Err(crate::ClientError::Network(format!(
                    "host wake HTTP {}",
                    response.status()
                )));
            }
            Ok(())
        }
        .await;
        if result.is_ok() && rows_flushed {
            match live.store.has_pending_chat_updates(chat) {
                Ok(false) => {
                    // This wake was posted AFTER the outgoing row ACKs. A
                    // wake sent before them can catch up to an empty room.
                    if let Some(version) = version {
                        if let Err(err) = live.store.complete_sync_job(chat, JOB, version) {
                            tracing::warn!(%err, %chat, "delivery receipt completion failed");
                        } else if live.store.sync_job_version(chat, JOB).ok() == Some(None) {
                            return;
                        }
                    } else {
                        return;
                    }
                }
                Ok(true) => {}
                Err(err) => tracing::warn!(%err, %chat, "delivery outbox check failed"),
            }
        } else if let Err(err) = result {
            tracing::debug!(%err, %chat, "host wake retrying");
        }
        let cancel = inner.cancel.clone();
        drop(inner);
        if !super::wait_backoff(&cancel, backoff).await {
            return;
        }
        backoff = (backoff * 2).min(super::JOIN_RETRY_CAP);
    }
}
