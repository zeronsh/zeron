//! Host RPCs over the device-room relay (`zeron_rpc::LinkCache`: one cached,
//! self-evicting link per target device, dark-peer fast-fail, dial cooldown),
//! plus the multi-call transfers built on it and the PR-status watches.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zeron_proto::CheckoutChangeRequestStatus;
use zeron_rpc::{LinkCache, LinkCacheConfig, PeerLiveness, RpcError, methods};

use super::{Bearer, b64};
use crate::client::ClientInner;
use crate::error::{ClientError, Result};
use crate::rpc::ProgressFn;
use crate::{lock, now_ms};

/// Default unary deadline (engine forward DEFAULT); long ones below.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Raw bytes per UploadChunk slice (≈680k base64 chars — under Cloudflare's
/// 1 MiB WS message cap with envelope headroom; a multiple of 3 so slices
/// encode independently).
const UPLOAD_SLICE_BYTES: usize = 510_000;
const UPLOAD_PARALLEL: usize = 3;
/// A device with presence older than this reads Dark (no dial) once the
/// registry has been live long enough to have heard its beats.
const DARK_AFTER_MS: i64 = 5 * 60_000;
const LIVENESS_WARMUP_MS: i64 = 60_000;

pub(crate) fn deadline(method: &str) -> Duration {
    // Mirrors the engine's forward_deadline tiers: a vendor installer or a
    // clone legitimately runs for minutes, and timing out here only hides a
    // result the host still delivers.
    match method {
        methods::INSTALL_HARNESS | methods::CLONE_REPO | methods::FETCH_ALL => {
            Duration::from_secs(15 * 60)
        }
        methods::APPLY_HARNESS_UPDATE => Duration::from_secs(20 * 60),
        methods::APPLY_ALL_HARNESS_UPDATES => Duration::from_secs(60 * 60),
        methods::UNINSTALL_HARNESS => Duration::from_secs(6 * 60),
        methods::CHECK_HARNESS_UPDATES => Duration::from_secs(4 * 60),
        methods::CREATE_WORKTREE => Duration::from_secs(120),
        methods::LIST_MODELS => Duration::from_secs(100),
        methods::UPLOAD_COMMIT => Duration::from_secs(150),
        _ => CALL_TIMEOUT,
    }
}

fn map_rpc(device_id: &str, method: &str, err: RpcError) -> ClientError {
    match err {
        RpcError::UnknownMethod(m) => ClientError::Unsupported(format!("{m} on {device_id}")),
        RpcError::BadParams(m) | RpcError::Failed(m) => {
            ClientError::HostError(format!("{method}: {m}"))
        }
        RpcError::Transport(m) => ClientError::HostUnavailable(format!("{device_id}: {m}")),
        RpcError::Closed => ClientError::HostUnavailable(format!("{device_id}: connection closed")),
    }
}

pub(crate) struct Relay {
    links: Arc<LinkCache>,
    /// The engine on this same device (`Credentials::Engine`): host RPCs
    /// addressed to our own device id go straight to its IPC port instead of
    /// a round trip through the edge's device room — the phone's terminal
    /// keystrokes and file reads stay on the phone.
    local: Option<LocalEngine>,
    watches: Mutex<HashMap<WatchKey, CancellationToken>>,
    /// Devices whose engine lacks WatchCheckoutChangeRequest (cleared on
    /// registry reconnect).
    unsupported: Mutex<HashSet<String>>,
}

struct LocalEngine {
    device_id: String,
    link: Arc<crate::engine::EngineLink>,
}

/// Deadline for a stream's acknowledgement (the host answers `{stream:true}`
/// before its first item, or fails the method).
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WatchKey {
    pub device_id: String,
    pub cwd: String,
    pub branch: String,
}

impl Relay {
    pub(crate) fn new(inner: &Arc<ClientInner>, edge: &str, bearer: Bearer) -> Self {
        let weak = Arc::downgrade(inner);
        let started = now_ms();
        let mut config = LinkCacheConfig::new(edge, Arc::new(bearer));
        config.liveness = Some(Arc::new(move |device_id: &str| {
            let Some(inner) = weak.upgrade() else {
                return PeerLiveness::Unknown;
            };
            let now = now_ms();
            match inner.workspace.presence().get(device_id) {
                Some(at) if now - at < crate::connectivity::PRESENCE_FRESH_MS => PeerLiveness::Live,
                seen => {
                    let registry_live = inner.live().is_some_and(|l| {
                        let posture = l.registry_posture();
                        posture.connected && posture.synced
                    });
                    let stale = seen.is_none_or(|at| now - at >= DARK_AFTER_MS);
                    if registry_live && stale && now - started >= LIVENESS_WARMUP_MS {
                        PeerLiveness::Dark
                    } else {
                        PeerLiveness::Unknown
                    }
                }
            }
        }));
        let links = {
            let _runtime = crate::runtime::handle().enter();
            LinkCache::new(config)
        };
        let local = match &inner.credentials {
            crate::config::Credentials::Engine {
                ipc_url, ipc_token, ..
            } => Some(LocalEngine {
                device_id: inner.config.device_id.clone(),
                link: crate::engine::EngineLink::new(ipc_url.clone(), ipc_token.clone()),
            }),
            _ => None,
        };
        Self {
            links,
            local,
            watches: Mutex::new(HashMap::new()),
            unsupported: Mutex::new(HashSet::new()),
        }
    }

    /// The IPC link when `device_id` is the engine this viewer shares.
    fn local_link(&self, device_id: &str) -> Option<&Arc<crate::engine::EngineLink>> {
        self.local
            .as_ref()
            .filter(|local| local.device_id == device_id)
            .map(|local| &local.link)
    }

    pub(crate) fn shutdown(&self) {
        self.links.disconnect_all();
        for (_, token) in lock(&self.watches).drain() {
            token.cancel();
        }
    }

    /// Presence says `device_id` is alive again: dial immediately next time.
    pub(crate) fn peer_alive(&self, device_id: &str) {
        self.links.reset_cooldown(device_id);
    }

    /// One unary call with a deadline; a stale cached link is retried once
    /// on a fresh dial.
    pub(crate) async fn call(&self, device_id: &str, method: &str, params: Value) -> Result<Value> {
        let timeout = deadline(method);
        if let Some(link) = self.local_link(device_id) {
            return match tokio::time::timeout(timeout, link.call_rpc(method, params)).await {
                Ok(result) => result.map_err(|e| map_rpc(device_id, method, e)),
                Err(_) => Err(ClientError::HostUnavailable(format!(
                    "{method} on {device_id} timed out"
                ))),
            };
        }
        for attempt in 0..2 {
            let client = self
                .links
                .client(device_id)
                .await
                .map_err(|e| map_rpc(device_id, method, e))?;
            match tokio::time::timeout(timeout, client.call(method, params.clone())).await {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(err @ (RpcError::Closed | RpcError::Transport(_)))) => {
                    self.links.invalidate(device_id);
                    if attempt == 1 {
                        return Err(map_rpc(device_id, method, err));
                    }
                }
                Ok(Err(err)) => return Err(map_rpc(device_id, method, err)),
                Err(_) => {
                    self.links.invalidate(device_id);
                    return Err(ClientError::HostUnavailable(format!(
                        "{method} on {device_id} timed out"
                    )));
                }
            }
        }
        unreachable!("the second attempt always returns")
    }

    /// A host stream (`SubscribeTerminal`, `WatchWorkspaceFiles`,
    /// `WatchPreviews`…), acknowledged by the host before it returns: an
    /// unknown method or a rejected request fails here. Dropping the
    /// subscription cancels the host's stream. A stale cached link is
    /// retried once on a fresh dial.
    pub(crate) async fn subscribe(
        &self,
        device_id: &str,
        method: &str,
        params: Value,
    ) -> Result<zeron_rpc::RpcSubscription> {
        let timed_out =
            || ClientError::HostUnavailable(format!("{method} on {device_id} timed out"));
        if let Some(link) = self.local_link(device_id) {
            return match tokio::time::timeout(SUBSCRIBE_TIMEOUT, link.subscribe_rpc(method, params))
                .await
            {
                Ok(result) => result.map_err(|e| map_rpc(device_id, method, e)),
                Err(_) => Err(timed_out()),
            };
        }
        for attempt in 0..2 {
            let client = self
                .links
                .client(device_id)
                .await
                .map_err(|e| map_rpc(device_id, method, e))?;
            let subscribe = client.subscribe_checked(method, params.clone());
            match tokio::time::timeout(SUBSCRIBE_TIMEOUT, subscribe).await {
                Ok(Ok(stream)) => return Ok(stream),
                Ok(Err(err @ (RpcError::Closed | RpcError::Transport(_)))) => {
                    self.links.invalidate(device_id);
                    if attempt == 1 {
                        return Err(map_rpc(device_id, method, err));
                    }
                }
                Ok(Err(err)) => return Err(map_rpc(device_id, method, err)),
                Err(_) => {
                    self.links.invalidate(device_id);
                    return Err(timed_out());
                }
            }
        }
        unreachable!("the second attempt always returns")
    }

    /// Chunked upload (`UploadChunk` ×N with `seq`, then `UploadCommit`).
    /// Returns the durable host path. Idempotent per `upload_id`.
    pub(crate) async fn upload(
        &self,
        device_id: &str,
        upload_id: &str,
        file_name: &str,
        data: &[u8],
        progress: Option<ProgressFn>,
    ) -> Result<String> {
        let slices: Vec<&[u8]> = if data.is_empty() {
            vec![&[][..]]
        } else {
            data.chunks(UPLOAD_SLICE_BYTES).collect()
        };
        let total = slices.len();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let report = |done: usize| {
            if let Some(progress) = &progress {
                progress((done as f64 / total as f64).min(0.99));
            }
        };
        report(0);
        let mut next = 0usize;
        while next < total {
            let batch: Vec<(usize, &[u8])> = slices
                .iter()
                .enumerate()
                .skip(next)
                .take(UPLOAD_PARALLEL)
                .map(|(i, s)| (i, *s))
                .collect();
            next += batch.len();
            let calls = batch.into_iter().map(|(seq, slice)| {
                let done = done.clone();
                let report = &report;
                async move {
                    let params =
                        json!({ "uploadId": upload_id, "seq": seq, "data": b64::encode(slice) });
                    let mut last = None;
                    for attempt in 0..3u64 {
                        if attempt > 0 {
                            tokio::time::sleep(Duration::from_millis(
                                50 * attempt * (seq as u64 + 1),
                            ))
                            .await;
                        }
                        match self
                            .call(device_id, methods::UPLOAD_CHUNK, params.clone())
                            .await
                        {
                            Ok(_) => {
                                let now =
                                    done.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
                                report(now);
                                return Ok(());
                            }
                            Err(
                                err @ (ClientError::Unsupported(_) | ClientError::HostError(_)),
                            ) => return Err(err),
                            Err(err) => last = Some(err),
                        }
                    }
                    Err(last.unwrap_or(ClientError::HostUnavailable(device_id.to_owned())))
                }
            });
            for result in futures::future::join_all(calls).await {
                result?;
            }
        }
        let reply = self
            .call(
                device_id,
                methods::UPLOAD_COMMIT,
                json!({ "uploadId": upload_id, "fileName": file_name }),
            )
            .await?;
        if let Some(progress) = &progress {
            progress(1.0);
        }
        reply
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ClientError::HostError("UploadCommit returned no path".into()))
    }

    /// `ReadAttachmentChunk` until done.
    pub(crate) async fn read_attachment(&self, device_id: &str, path: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut offset: u64 = 0;
        for _ in 0..1000 {
            let reply = self
                .call(
                    device_id,
                    methods::READ_ATTACHMENT_CHUNK,
                    json!({ "path": path, "offset": offset }),
                )
                .await?;
            let data = reply
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            out.extend(
                b64::decode(data)
                    .ok_or_else(|| ClientError::HostError("bad attachment chunk".into()))?,
            );
            if reply.get("done").and_then(Value::as_bool).unwrap_or(true) {
                return Ok(out);
            }
            let next = reply
                .get("nextOffset")
                .and_then(Value::as_u64)
                .unwrap_or(offset);
            if next <= offset {
                return Err(ClientError::HostError(
                    "attachment read made no progress".into(),
                ));
            }
            offset = next;
        }
        Err(ClientError::HostError("attachment too large".into()))
    }

    pub(crate) fn clear_unsupported(&self) {
        lock(&self.unsupported).clear();
    }

    /// Converge the running PR-status streams onto `targets`.
    pub(crate) fn reconcile_watches(&self, inner: &Arc<ClientInner>, targets: HashSet<WatchKey>) {
        let unsupported = lock(&self.unsupported).clone();
        let mut watches = lock(&self.watches);
        watches.retain(|key, token| {
            let keep = targets.contains(key) && !unsupported.contains(&key.device_id);
            if !keep {
                token.cancel();
            }
            keep
        });
        for key in targets {
            if watches.contains_key(&key) || unsupported.contains(&key.device_id) {
                continue;
            }
            let token = CancellationToken::new();
            watches.insert(key.clone(), token.clone());
            spawn_watch(Arc::downgrade(inner), key, token);
        }
    }

    pub(crate) fn stop_watches(&self) {
        for (_, token) in lock(&self.watches).drain() {
            token.cancel();
        }
    }
}

/// One `WatchCheckoutChangeRequest` stream, retried 0.5s→5s forever (last
/// snapshot kept through gaps); an engine without the method marks the
/// device unsupported.
fn spawn_watch(weak: Weak<ClientInner>, key: WatchKey, cancel: CancellationToken) {
    crate::runtime::shared().spawn(async move {
        let mut backoff = Duration::from_millis(500);
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let Some(inner) = weak.upgrade() else { return };
            let Some(live) = inner.live() else { return };
            let links = live.relay.links.clone();
            drop(inner);
            let result = async {
                let client = links.client(&key.device_id).await?;
                client
                    .subscribe_checked(
                        methods::WATCH_CHECKOUT_CHANGE_REQUEST,
                        json!({ "cwd": key.cwd, "branch": key.branch }),
                    )
                    .await
            };
            let subscription = tokio::select! {
                _ = cancel.cancelled() => return,
                result = result => result,
            };
            match subscription {
                Ok(mut stream) => loop {
                    let item = tokio::select! {
                        _ = cancel.cancelled() => return,
                        item = stream.recv() => item,
                    };
                    let Some(item) = item else { break };
                    let Ok(status) = serde_json::from_value::<CheckoutChangeRequestStatus>(item)
                    else {
                        continue;
                    };
                    if status.device_id != key.device_id || status.cwd != key.cwd {
                        continue;
                    }
                    backoff = Duration::from_millis(500);
                    let Some(inner) = weak.upgrade() else { return };
                    inner.workspace.put_change_request(status);
                    inner.recompute_workspace();
                },
                Err(RpcError::UnknownMethod(_)) => {
                    if let Some(inner) = weak.upgrade()
                        && let Some(live) = inner.live()
                    {
                        lock(&live.relay.unsupported).insert(key.device_id.clone());
                    }
                    return;
                }
                Err(RpcError::Closed | RpcError::Transport(_)) => links.invalidate(&key.device_id),
                Err(_) => {}
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vendor installer runs for minutes; a 30s client deadline reported
    /// failure while the host was still installing (Android Agents panel).
    #[test]
    fn long_host_operations_outlive_the_default_deadline() {
        for method in [
            methods::INSTALL_HARNESS,
            methods::CLONE_REPO,
            methods::FETCH_ALL,
        ] {
            assert_eq!(deadline(method), Duration::from_secs(15 * 60), "{method}");
        }
        assert_eq!(deadline(methods::LIST_REFS), CALL_TIMEOUT);
    }

    /// Harness maintenance mirrors the engine's forward deadlines: an
    /// update-all pass or an npm uninstall must not read as a failure while
    /// the host is still working.
    #[test]
    fn harness_maintenance_deadlines_match_the_engine_tiers() {
        assert_eq!(
            deadline(methods::APPLY_ALL_HARNESS_UPDATES),
            Duration::from_secs(60 * 60)
        );
        assert_eq!(
            deadline(methods::UNINSTALL_HARNESS),
            Duration::from_secs(6 * 60)
        );
        assert_eq!(
            deadline(methods::CHECK_HARNESS_UPDATES),
            Duration::from_secs(4 * 60)
        );
        assert_eq!(deadline(methods::LIST_HARNESS_UPDATES), CALL_TIMEOUT);
    }
}
