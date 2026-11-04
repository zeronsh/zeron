//! Read-only transcript projection into the owner-scoped Scaffold directory.
//! Registry sequence numbers fence acknowledged fields; Loro VV fences links.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use loro::{Container, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_sync::DocsStore;

use crate::{DocHost, EngineError, WorkspaceHost, auth::{Auth, AuthState}, crew::CrewServices, scaffold::ScaffoldError};

type Version = BTreeMap<String, u64>;
const JOB: &str = "crew-directory";
const REGISTRY_VERSION: &str = "r:registry1";

const RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
enum PublishError {
    #[error(transparent)]
    Row(#[from] EngineError),
    #[error("directory {operation}: {source}")]
    Remote { operation: &'static str, source: ScaffoldError },
}

// Elapsed time avoids overflowing Instant for a very large Retry-After. Sleep in
// bounded chunks without doing any work or allowing notifications to cut it short.
async fn wait_for_retry(started: tokio::time::Instant, delay: Duration) {
    loop {
        let remaining = delay.saturating_sub(started.elapsed());
        if remaining.is_zero() { return; }
        tokio::time::sleep(remaining.min(Duration::from_secs(24 * 60 * 60))).await;
    }
}

async fn wait_for_work(
    registry: &mut tokio::sync::watch::Receiver<u64>,
    snapshots: &mut tokio::sync::watch::Receiver<u64>,
    identity: &mut tokio::sync::watch::Receiver<AuthState>,
    cooldown: Option<Duration>,
) -> Option<bool> {
    let started = tokio::time::Instant::now();
    let mut inventory = false;
    loop {
        tokio::select! {
            biased;
            _ = wait_for_retry(started, cooldown.unwrap_or_default()), if cooldown.is_some() => return Some(inventory),
            result = registry.changed() => { if result.is_err() { return None; } inventory = true; },
            result = snapshots.changed() => { if result.is_err() { return None; } },
            result = identity.changed() => { if result.is_err() { return None; } inventory = true; },
        }
        if cooldown.is_none() { return Some(inventory); }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Evidence {
    source_version: Version,
    confirmed: bool,
    // Suppress only the rejected local candidate, never adopt remote evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rejected_source_version: Option<Version>,
}

fn failure(message: &str) -> EngineError { EngineError::Other(message.into()) }

/// One bounded worker. The engine owns/aborts this handle before profile shutdown.
/// No chat is opened, attached, claimed, or executed to make it discoverable.
pub(crate) fn start(auth: Auth, services: CrewServices, workspace: WorkspaceHost,
    doc_host: DocHost) -> tokio::task::JoinHandle<()> {
    let store = doc_host.crew_store();
    let mut registry = workspace.watch_registry();
    let mut snapshots = store.watch_directory();
    let mut identity = auth.watch_state();
    tokio::spawn(async move {
        let mut inventory = true;
        // Remote versions are prerequisites only, never local projection evidence.
        let mut required_sources = BTreeMap::new();
        loop {
            inventory |= registry.has_changed().unwrap_or(false) || identity.has_changed().unwrap_or(false);
            registry.borrow_and_update();
            snapshots.borrow_and_update();
            identity.borrow_and_update();
            let mut cooldown = RETRY_DELAY;
            if services.ensure_identity().is_ok() && workspace.registry_synced() {
                if inventory {
                    if let Err(error) = enqueue_registry(&store, &workspace).await {
                        tracing::warn!(%error, "directory registry inventory retained for retry");
                    } else {
                        inventory = false;
                    }
                }
                match drain(&store, &services, &workspace, &mut required_sources).await {
                    Ok(Some(delay)) => cooldown = delay,
                    Ok(None) => {},
                    Err(error) => tracing::warn!(%error, "directory pending work retained for retry"),
                }
            }
            // Notifications accumulate inventory but cannot bypass backpressure.
            let pending = store.pending_sync_jobs(JOB, "", 1).map_or(true, |ids| !ids.is_empty());
            let Some(changed) = wait_for_work(&mut registry, &mut snapshots, &mut identity,
                (pending || inventory).then_some(cooldown)).await else { return };
            inventory |= changed;
        }
    })
}

async fn enqueue_registry(store: &DocsStore, workspace: &WorkspaceHost) -> Result<(), EngineError> {
    let mut after = String::new();
    loop {
        let rows = workspace.acknowledged_chats_after(&after);
        if rows.is_empty() { return Ok(()); }
        for row in rows {
            after = row.id;
            if zeron_proto::is_cloud_session_id(&after) { store.queue_directory(&after)?; }
        }
        tokio::task::yield_now().await;
    }
}

/// Older builds also raised the registry reset flag from stale answers. Clear
/// it once the fences this device sent show nothing was renumbered (see
/// `RegistryDoc::clear_directory_reconciliation`); after a real reset it stays.
fn disprove_registry_reset(store: &DocsStore, workspace: &WorkspaceHost) -> Result<(), EngineError> {
    let mut sent = BTreeMap::new();
    let mut after = String::new();
    loop {
        let rows = workspace.acknowledged_chats_after(&after);
        if rows.is_empty() { break; }
        for row in rows {
            after = row.id;
            let Some(saved) = store.directory_state(&after)? else { continue };
            // Evidence that cannot be read cannot show what was sent: keep the flag.
            let Ok(evidence) = serde_json::from_str::<Evidence>(&saved) else { return Ok(()) };
            if let Some(fence) = evidence.source_version.get(REGISTRY_VERSION) {
                sent.insert(after.clone(), *fence);
            }
        }
    }
    if workspace.clear_directory_reconciliation(|id| sent.get(id).copied()) {
        PAUSE_REPORTED.store(false, Ordering::Relaxed);
        tracing::info!(chats = sent.len(), "crew directory: registry reset flag cleared; publication resumes");
    }
    Ok(())
}

/// Whether this process has reported that publication is paused by the flag.
static PAUSE_REPORTED: AtomicBool = AtomicBool::new(false);

async fn drain(store: &Arc<DocsStore>, services: &CrewServices, workspace: &WorkspaceHost,
    required_sources: &mut BTreeMap<String, Version>) -> Result<Option<Duration>, EngineError> {
    if workspace.directory_reconciliation_required() {
        disprove_registry_reset(store, workspace)?;
        if workspace.directory_reconciliation_required() {
            // Every publish would refuse, each with a warning; keep the jobs instead.
            if !PAUSE_REPORTED.swap(true, Ordering::Relaxed) {
                tracing::warn!("crew directory: publication paused until the registry reset flag clears");
            }
            return Ok(None);
        }
    }
    let mut after = String::new();
    loop {
        let ids = store.pending_sync_jobs(JOB, &after, 64)?;
        if ids.is_empty() { return Ok(None); }
        for id in ids {
            after.clone_from(&id);
            services.ensure_identity()?;
            let Some(generation) = store.sync_job_version(&id, JOB)? else { continue };
            match publish(store, services, workspace, &id, required_sources).await {
                Ok(true) => {
                    store.complete_sync_job(&id, JOB, generation)?;
                    required_sources.remove(&id);
                }
                Ok(false) => {}, // Keep the durable job until its own source catches up.
                Err(error) => {
                    tracing::warn!(session = %id, %error, "directory update awaits synchronization or reconciliation");
                    if let PublishError::Remote { source, .. } = error
                        && source.is_transient() {
                        return Ok(Some(source.retry_after().unwrap_or(RETRY_DELAY).max(RETRY_DELAY)));
                    }
                },
            }
        }
        tokio::task::yield_now().await;
    }
}

async fn publish(store: &Arc<DocsStore>, services: &CrewServices, workspace: &WorkspaceHost,
    id: &str, required_sources: &mut BTreeMap<String, Version>) -> Result<bool, PublishError> {
    if !zeron_proto::is_cloud_session_id(id) { return Ok(true); }
    if workspace.directory_reconciliation_required() {
        return Err(failure("registry server reset observed; directory reconciliation required").into());
    }
    let Some(row) = workspace.acknowledged_chat(id) else {
        // Absence is not a deletion; optimistic creations await their server ACK.
        return Err(failure("directory awaits acknowledged registry row").into());
    };
    if row.seq == 0 { return Err(failure("directory awaits server registry sequence").into()); }
    let expected = services.session_metadata(id)?;
    let deployment = expected.scope.deployment_id.as_deref()
        .ok_or_else(|| failure("directory requires exact deployment"))?;
    if deployment != services.runtime().deployment_id() {
        return Err(failure("directory runtime differs from verified deployment").into());
    }
    if !row.deleted {
        // Phones create crew:None. Stamp only through the shared, identity-checked
        // adoption path, then wait for the server to acknowledge that metadata.
        services.adopt_chat(workspace, id)?;
        let metadata: zeron_proto::CrewChatMetadata = serde_json::from_value(
            row.fields.get("crew").cloned().unwrap_or(Value::Null))
            .map_err(|_| failure("directory awaits acknowledged Crew metadata"))?;
        metadata.validate().map_err(failure)?;
        if metadata.owner_subject != expected.owner_subject || metadata.scope != expected.scope
            || metadata.session_id != id {
            return Err(failure("directory owner or scope mismatch").into());
        }
    }
    let mut evidence: Evidence = match store.directory_state(id).map_err(EngineError::from)? {
        Some(saved) => serde_json::from_str(&saved).map_err(|_| failure("invalid directory evidence"))?,
        None => Evidence::default(),
    };
    // A deletion can use only previously observed local transcript evidence,
    // never counters copied out of a directory response.
    let projection_store = store.clone();
    let projection_id = id.to_owned();
    let projection = tokio::task::spawn_blocking(move || {
        projection_store.load_directory_snapshot(&projection_id)?
            .map(|bytes| project(&bytes)).transpose()
    }).await.map_err(|_| failure("directory snapshot projection failed"))??;
    let (mut version, mut links) = match projection {
        Some(projection) => projection,
        None if row.deleted => (evidence.source_version.clone(), Vec::new()),
        // Registry-only rows are normal on controllers of remotely hosted chats.
        // Keep their jobs pending until a local transcript snapshot is saved.
        None => return Ok(false),
    };
    if row.deleted { links.clear(); }
    version.insert(REGISTRY_VERSION.into(), row.seq);
    require_observed(&version, &Version::new())?;
    if require_observed(&version, &evidence.source_version).is_err()
        || required_sources.get(id).is_some_and(|required| require_observed(&version, required).is_err()) {
        return Ok(false);
    }
    if evidence.rejected_source_version.as_ref() == Some(&version) { return Ok(false); }
    if evidence.confirmed && evidence.source_version == version { return Ok(true); }
    services.ensure_identity()?;
    let client = services.runtime().client();
    let remote = client.get_crew_session(id, Some(deployment)).await
        .map_err(|source| PublishError::Remote { operation: "revision lookup", source })?;
    let previous = remote.get("session").ok_or_else(|| failure("invalid directory response"))?;
    let revision = if !previous.is_null() {
        let observed: Version = serde_json::from_value(previous["sourceVersion"].clone())
            .map_err(|_| failure("invalid directory server version"))?;
        require_observed(&observed, &Version::new())?;
        if let Err(error) = require_observed(&version, &observed) {
            required_sources.insert(id.to_owned(), observed);
            evidence.rejected_source_version = Some(version.clone());
            store.save_directory_state(id, &serde_json::to_string(&evidence)
                .map_err(|_| failure("invalid directory evidence"))?).map_err(EngineError::from)?;
            return Err(error.into());
        }
        if previous.get("deleted").and_then(Value::as_bool) == Some(true) { return Ok(true); }
        Some(previous["revision"].as_u64().filter(|revision| *revision > 0)
            .ok_or_else(|| failure("invalid directory server revision"))?)
    } else { None };
    // Do not send a projection captured before a registry ACK arriving during
    // HTTP lookup. Concurrent later writers still meet expectedRevision at SQL.
    if workspace.directory_reconciliation_required() {
        return Err(failure("registry server reset observed; directory reconciliation required").into());
    }
    if workspace.acknowledged_chat(id).as_ref() != Some(&row) {
        return Err(failure("directory registry changed during projection").into());
    }
    evidence.rejected_source_version = None;
    evidence.source_version = version;
    evidence.confirmed = false;
    store.save_directory_state(id, &serde_json::to_string(&evidence)
        .map_err(|_| failure("invalid directory evidence"))?).map_err(EngineError::from)?;
    services.ensure_identity()?;
    // Platform authorizes upsert via the read-only existing-chat2 owner endpoint.
    // Empty new chats with no room owner remain pending; this never creates one.
    let ack = client.upsert_crew_session(&json!({
        "sessionId": id, "deploymentId": deployment, "expectedRevision": revision,
        "sourceVersion": evidence.source_version, "generatedTitle": null,
        "title": row.fields.get("title").and_then(Value::as_str).unwrap_or_default()
            .chars().take(140).collect::<String>(),
        "links": links, "deleted": row.deleted,
    })).await.map_err(|source| PublishError::Remote { operation: "upsert", source })?;
    if ack["revision"].as_u64().filter(|revision| *revision > 0).is_none() {
        return Err(failure("invalid directory acknowledgement").into());
    }
    evidence.confirmed = true;
    store.save_directory_state(id, &serde_json::to_string(&evidence)
        .map_err(|_| failure("invalid directory evidence"))?).map_err(EngineError::from)?;
    Ok(true)
}

fn require_observed(candidate: &Version, previous: &Version) -> Result<(), EngineError> {
    if candidate.len() > 128 || candidate.values().any(|counter| *counter > 9_007_199_254_740_991) {
        return Err(failure("directory source version exceeds wire bounds"));
    }
    if candidate.get(REGISTRY_VERSION).copied().unwrap_or(0)
        < previous.get(REGISTRY_VERSION).copied().unwrap_or(0) {
        return Err(failure("registry sequence regressed; directory reconciliation required"));
    }
    if previous.iter().any(|(peer, counter)| candidate.get(peer).copied().unwrap_or(0) < *counter) {
        return Err(failure("directory source is older or concurrent; awaiting synchronized snapshot"));
    }
    Ok(())
}

fn field_string(map: &loro::LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(value))) => Some(value.to_string()),
        _ => None,
    }
}

/// Original Crew source-link extraction, without title inference or full-history
/// materialization. Canonical Slack/Notion keys are shared by server upsert/search.
fn project(bytes: &[u8]) -> Result<(Version, Vec<String>), EngineError> {
    let doc = loro::LoroDoc::new();
    doc.import(bytes).map_err(|_| failure("invalid directory snapshot"))?;
    let version = doc.oplog_vv().iter()
        .map(|(peer, counter)| (format!("s:{peer}"), *counter as u64)).collect();
    let mut links = BTreeSet::new();
    let messages = doc.get_list("messages");
    for index in 0..messages.len() {
        let Some(ValueOrContainer::Container(Container::Map(row))) = messages.get(index) else {
            return Err(failure("malformed directory transcript row"));
        };
        if field_string(&row, "role").as_deref() == Some("system") { continue; }
        let Some(ValueOrContainer::Container(Container::List(parts))) = row.get("parts") else { continue };
        for index in 0..parts.len() {
            let Some(ValueOrContainer::Container(Container::Map(part))) = parts.get(index) else { continue };
            match field_string(&part, "kind").as_deref() {
                Some("text") => {
                    let mut carry = String::new();
                    match part.get("text") {
                        Some(ValueOrContainer::Container(Container::Text(text))) => {
                            let mut start = 0;
                            while start < text.len_utf8() {
                                let mut end = (start + 64 * 1024).min(text.len_utf8());
                                let delta = loop {
                                    match text.slice_delta(start, end, loro::cursor::PosType::Bytes) {
                                        Ok(delta) => break delta,
                                        Err(_) if end > start + 1 => end -= 1,
                                        Err(_) => return Err(failure("invalid directory text boundary")),
                                    }
                                };
                                for delta in delta {
                                    if let loro::TextDelta::Insert { insert, .. } = delta {
                                        scan_chunk(&mut carry, &insert, false, &mut links)?;
                                    }
                                }
                                start = end;
                            }
                        }
                        Some(ValueOrContainer::Value(LoroValue::String(text))) => {
                            extract_links(&text, &mut links)?;
                        }
                        _ => {}
                    }
                    scan_chunk(&mut carry, "", true, &mut links)?;
                }
                Some("tool") => {
                    if let Some(ValueOrContainer::Value(value)) = part.get("call") {
                        tool_links(&value, &mut links, 0)?;
                    }
                }
                _ => {},
            }
        }
    }
    Ok((version, links.into_iter().collect()))
}

fn tool_links(value: &LoroValue, links: &mut BTreeSet<String>, depth: usize) -> Result<(), EngineError> {
    if depth > 64 { return Err(failure("directory tool content exceeds nesting bound")); }
    match value {
        LoroValue::String(value) => extract_links(value, links)?,
        LoroValue::List(items) => for value in items.iter() { tool_links(value, links, depth + 1)?; },
        LoroValue::Map(fields) => for (key, value) in fields.iter() {
            if !matches!(key.as_str(), "reasoning" | "thinking" | "system" | "systemPrompt") {
                tool_links(value, links, depth + 1)?;
            }
        },
        _ => {},
    }
    Ok(())
}

fn url_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`' | '|' | ')' | ']')
}

fn scan_chunk(carry: &mut String, chunk: &str, finished: bool, links: &mut BTreeSet<String>)
    -> Result<(), EngineError> {
    carry.push_str(chunk);
    let split = if finished { carry.len() } else {
        carry.char_indices().rev().find(|(_, ch)| url_separator(*ch))
            .map_or(0, |(index, ch)| index + ch.len_utf8())
    };
    extract_links(&carry[..split], links)?;
    carry.drain(..split);
    if carry.len() > 4096 {
        extract_links(carry, links)?;
        let mut suffix = carry.len().saturating_sub(8);
        while !carry.is_char_boundary(suffix) { suffix += 1; }
        carry.drain(..suffix);
    }
    Ok(())
}

fn extract_links(text: &str, links: &mut BTreeSet<String>) -> Result<(), EngineError> {
    for (start, _) in text.match_indices("https://").chain(text.match_indices("http://")) {
        let candidate = text[start..].split(url_separator).next().unwrap_or_default()
            .trim_end_matches(['.', ',', ';', ':']);
        let Ok(url) = reqwest::Url::parse(candidate) else { continue };
        let host = url.host_str().unwrap_or_default();
        if (host.ends_with(".slack.com") && url.path().starts_with("/archives/"))
            || host == "notion.so" || host.ends_with(".notion.so")
            || host == "notion.site" || host.ends_with(".notion.site") {
            if candidate.len() > 4096 { return Err(failure("directory URL exceeds scan budget")); }
            links.insert(candidate.to_string());
            if links.len() > 512 || links.iter().map(String::len).sum::<usize>() > 48 * 1024 {
                return Err(failure("directory links exceed request budget"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, convert::Infallible};
    use parking_lot::Mutex;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::{Request, Response, body::Incoming, service::service_fn};
    use hyper_util::rt::TokioIo;
    use crate::{auth::AuthConfig, scaffold::{ScaffoldClient, ScaffoldRuntime,
        UnavailableDeviceJoinGrantProvider}, workspace_host::WorkspaceHostConfig};
    use zeron_doc::{RegistryDoc, REGISTRY_DOC_ID, registry::{RegistryRow, KIND_CHATS}};
    use zeron_sync::scaffold_auth::{save_session, ScaffoldConfig, ScaffoldIdentity,
        ScaffoldSession, DEFAULT_OAUTH_SCOPES};

    type Replies = Arc<Mutex<VecDeque<(u16, Value)>>>;
    type Recorded = Arc<Mutex<Vec<(String, Value)>>>;

    /// A Scaffold double for alice: it records directory requests and answers
    /// them from `replies` in order (500 once they run out).
    async fn fake_scaffold(replies: Replies, requests: Recorded) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn({
            let origin = origin.clone();
            async move {
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    let origin = origin.clone();
                    let replies = replies.clone();
                    let requests = requests.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |request: Request<Incoming>| {
                            let origin = origin.clone();
                            let replies = replies.clone();
                            let requests = requests.clone();
                            async move {
                                let path = request.uri().path().to_owned();
                                let body = request.into_body().collect().await.unwrap().to_bytes();
                                let (status, response) = match path.as_str() {
                                    "/auth/session" => (200, json!({"userId":"alice", "email":"alice@example.com",
                                        "projectScope":"project-a", "deploymentId":"deployment-a",
                                        "credential":"scaffold", "capabilities":["session.read"]})),
                                    "/api/code-sandboxes/auth/session" => (200, json!({"ok":true,
                                        "resource":origin, "actor":{"sub":"alice", "auth":"iap"},
                                        "scopes":DEFAULT_OAUTH_SCOPES.split_whitespace().collect::<Vec<_>>()})),
                                    "/api/crew-directory/get" | "/api/crew-directory/upsert" => {
                                        requests.lock().push((path, serde_json::from_slice(&body).unwrap()));
                                        replies.lock().pop_front().unwrap_or((500, json!({})))
                                    }
                                    _ => panic!("unexpected request: {path}"),
                                };
                                Ok::<_, Infallible>(Response::builder().status(status)
                                    .header("content-type", "application/json").header("connection", "close")
                                    .body(Full::new(Bytes::from(response.to_string()))).unwrap())
                            }
                        });
                        hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service).await.unwrap();
                    });
                }
            }
        });
        (origin, server)
    }

    /// Crew services signed in as alice (project-a, deployment-a), and their store.
    async fn signed_in_services(dir: &std::path::Path, origin: &str) -> (CrewServices, Arc<DocsStore>) {
        save_session(&dir.join("scaffold-session.json"), &ScaffoldSession {
            access_token: "sc_rc_test".into(), resource: origin.into(), edge_origin: origin.into(), device: None,
            identity: ScaffoldIdentity { user_id: "alice".into(), email: "alice@example.com".into(), name: None,
                project_scope: "project-a".into(), deployment_id: Some("deployment-a".into()),
                capabilities: vec!["session.read".into()] },
        }).unwrap();
        let mut config = AuthConfig::new(origin, dir);
        config.scaffold = Some(ScaffoldConfig::new(origin, origin, "project-a"));
        let auth = Auth::detect(config).await;
        let runtime = ScaffoldRuntime::new(
            ScaffoldClient::new(origin, "project-a", auth.for_profile_tokens()).unwrap(),
            origin, Arc::new(UnavailableDeviceJoinGrantProvider),
        ).with_deployment_id("deployment-a".into());
        let store = Arc::new(DocsStore::open(&dir.join("store")).unwrap());
        (CrewServices::new(auth, runtime, store.clone()).unwrap(), store)
    }

    fn open_workspace(store: &Arc<DocsStore>) -> WorkspaceHost {
        WorkspaceHost::open(store.clone(), WorkspaceHostConfig {
            device_id: "test-device".into(), device_name: "Test".into(), platform: "linux".into(),
            org_id: "project-a".into(), user_id: "alice".into(), edge: None, local_only: false,
            owner_registry: true,
        }).unwrap()
    }

    /// The acknowledged registry row of a chat this device hosts, at `seq`.
    fn chat_row(services: &CrewServices, id: &str, seq: u64) -> RegistryRow {
        RegistryRow {
            kind: KIND_CHATS.into(), id: id.into(), seq, deleted: false, del_hlc: None,
            fields: BTreeMap::from([("id".into(), json!(id)), ("deviceId".into(), json!("test-device")),
                ("crew".into(), serde_json::to_value(services.session_metadata(id).unwrap()).unwrap())]),
            clocks: BTreeMap::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn signal_storm_cannot_shorten_or_extend_worker_cooldown() {
        for delay in [RETRY_DELAY, Duration::from_secs(125)] {
            let (registry_tx, mut registry) = tokio::sync::watch::channel(0);
            let (snapshots_tx, mut snapshots) = tokio::sync::watch::channel(0);
            let (identity_tx, mut identity) = tokio::sync::watch::channel(AuthState::SignedOut);
            // Include the inventory's own wake already pending before waiting.
            snapshots_tx.send_replace(1);
            let waiter = tokio::spawn(async move {
                wait_for_work(&mut registry, &mut snapshots, &mut identity, Some(delay)).await
            });
            tokio::task::yield_now().await;
            for index in 1..delay.as_secs() {
                registry_tx.send_replace(index);
                snapshots_tx.send_replace(index);
                identity_tx.send_replace(AuthState::SignedOut);
                tokio::time::advance(Duration::from_secs(1)).await;
                tokio::task::yield_now().await;
                assert!(!waiter.is_finished(), "notification bypassed cooldown");
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            assert_eq!(waiter.await.unwrap(), Some(true));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unrepresentable_deadline_never_overflows_or_retries_early() {
        let started = tokio::time::Instant::now();
        let waiter = tokio::spawn(wait_for_retry(started, Duration::MAX));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2 * 24 * 60 * 60)).await;
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        waiter.abort();
    }


    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registry_only_jobs_wait_for_snapshot_and_preserve_publication_fences() {
        let id = "9dd173da-fb53-48df-9581-026009c783e2";
        let doc = loro::LoroDoc::new();
        doc.set_peer_id(42).unwrap();
        let row = doc.get_list("messages").push_container(loro::LoroMap::new()).unwrap();
        row.insert("role", "user").unwrap();
        let parts = row.insert_container("parts", loro::LoroList::new()).unwrap();
        let part = parts.push_container(loro::LoroMap::new()).unwrap();
        part.insert("kind", "text").unwrap();
        let text = part.insert_container("text", loro::LoroText::new()).unwrap();
        text.insert(0, "Investigate this incident: ").unwrap();
        doc.commit();
        let stale_snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        let url = "https://acme.slack.com/archives/C123/p1234567890123456";
        text.insert(text.len_unicode(), url).unwrap();
        doc.commit();
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        let (mut version, _) = project(&snapshot).unwrap();
        version.insert(REGISTRY_VERSION.into(), 9);
        let remote = json!({"session":{"revision":7, "sourceVersion":version, "deleted":false}});
        let responses = Arc::new(Mutex::new(VecDeque::from([
            (200_u16, remote.clone()), // First stale source.
            (503, json!({})), // Catch-up is not suppressed after a failed GET.
            (200, remote.clone()), (503, json!({})), // Nor after a failed upsert.
            (200, remote), (200, json!({"revision":8})),
        ])));
        let requests = Recorded::default();
        let (origin, server) = fake_scaffold(responses.clone(), requests.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let (services, store) = signed_in_services(dir.path(), &origin).await;
        let mut registry = RegistryDoc::new("test-device");
        let _ = registry.apply_state(9, true, 0, vec![chat_row(&services, id, 9)]);
        store.save_snapshot(REGISTRY_DOC_ID, &registry.to_bytes().unwrap()).unwrap();
        let workspace = open_workspace(&store);
        let local_id = "local-chat-0123456789abcdef";
        workspace.create_chat(local_id, None, Some("test-device"), Some(zeron_proto::ChatConfig {
            harness: zeron_proto::HarnessId::Omp, model: Some("original-model".into()),
            reasoning: None, model_options: Default::default(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite, access: Default::default(), plan: false,
        }), Some("/original/repo".into())).unwrap();
        workspace.set_chat_harness_session(
            local_id,
            "original-native",
            "/original/repo",
            Some(zeron_proto::HarnessId::Omp),
        );
        let local = services.adopt_chat(&workspace, local_id).unwrap();
        assert_eq!(local.id, local_id);
        assert_eq!(local.harness_session_id.as_deref(), Some("original-native"));
        assert!(local.crew.is_none());
        assert!(services.session_metadata(local_id).is_err());
        assert!(services.generate_title(local_id, "local prompt").await.is_err());
        assert!(publish(&store, &services, &workspace, local_id, &mut BTreeMap::new()).await.unwrap());
        assert!(requests.lock().is_empty());
        let mut fabricated = local;
        fabricated.crew = Some(services.session_metadata(id).unwrap());
        workspace.import_chat_row(&fabricated).unwrap();
        assert!(services.adopt_chat(&workspace, local_id).is_err());
        let client = services.runtime().client();
        assert!(client.get_crew_session(local_id, Some("deployment-a")).await.is_err());
        assert!(client.upsert_crew_session(&json!({"sessionId":local_id})).await.is_err());
        assert!(client.generate_crew_title(local_id, Some("deployment-a"), "private").await.is_err());
        assert!(requests.lock().is_empty());
        let mut required = BTreeMap::new();
        enqueue_registry(&store, &workspace).await.unwrap();
        let pending = store.sync_job_version(id, JOB).unwrap().unwrap();
        assert!(store.load_directory_snapshot(id).unwrap().is_none());
        for _ in 0..2 {
            assert_eq!(drain(&store, &services, &workspace, &mut required).await.unwrap(), None);
            assert!(requests.lock().is_empty(), "registry-only rows must not issue directory HTTP");
            assert_eq!(store.sync_job_version(id, JOB).unwrap(), Some(pending));
            assert!(store.directory_state(id).unwrap().is_none());
            assert!(required.is_empty());
        }
        // A locally persisted transcript unblocks projection, but cannot bypass
        // the existing remote-version fence while it still lacks later edits.
        store.save_snapshot(id, &stale_snapshot).unwrap();
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert_eq!(requests.lock().len(), 1);
        assert_eq!(required[id], version);
        let rejected: Evidence = serde_json::from_str(&store.directory_state(id).unwrap().unwrap()).unwrap();
        assert!(rejected.source_version.is_empty(), "remote counters are not evidence");
        assert!(rejected.rejected_source_version.is_some());

        for _ in 0..3 {
            // The worker inventories on any registry wake and drains on any snapshot wake.
            enqueue_registry(&store, &workspace).await.unwrap();
            store.save_snapshot("unrelated", &snapshot).unwrap();
            drain(&store, &services, &workspace, &mut required).await.unwrap();
            assert_eq!(requests.lock().len(), 1);
            assert!(store.sync_job_version(id, JOB).unwrap().is_some());
        }
        required.clear(); // Persisted candidate rejection also survives worker restart.
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert_eq!(requests.lock().len(), 1);
        store.save_snapshot(id, &snapshot).unwrap();
        for expected_requests in [2, 4] {
            assert_eq!(drain(&store, &services, &workspace, &mut required).await.unwrap(), Some(RETRY_DELAY));
            assert_eq!(requests.lock().len(), expected_requests);
            assert!(store.sync_job_version(id, JOB).unwrap().is_some());
        }
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert_eq!(requests.lock().len(), 6);
        assert!(responses.lock().is_empty());
        assert!(store.sync_job_version(id, JOB).unwrap().is_none());
        assert!(required.is_empty());
        {
            let requests = requests.lock();
            let published = &requests[5];
            assert_eq!(published.0, "/api/crew-directory/upsert");
            assert_eq!(published.1["sourceVersion"], json!(version));
            assert_eq!(published.1["expectedRevision"], 7);
            assert_eq!(published.1["links"], json!([url]));
            assert_eq!(published.1["deleted"], false);
        }
        let saved = store.directory_state(id).unwrap().unwrap();
        let evidence: Evidence = serde_json::from_str(&saved).unwrap();
        assert!(evidence.confirmed);
        assert_eq!(evidence.source_version, version);

        // Losing the local snapshot must wait on saved evidence without any HTTP.
        store.delete_snapshot(id).unwrap();
        store.queue_directory(id).unwrap();
        for _ in 0..2 {
            drain(&store, &services, &workspace, &mut required).await.unwrap();
            assert!(store.sync_job_version(id, JOB).unwrap().is_some());
            assert_eq!(requests.lock().len(), 6);
        }
        store.save_snapshot(id, &snapshot).unwrap();
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert!(store.sync_job_version(id, JOB).unwrap().is_none());
        assert_eq!(requests.lock().len(), 6);
        assert_eq!(store.directory_state(id).unwrap().unwrap(), saved);
        workspace.shutdown();

        // Acknowledged deletion still publishes from prior local evidence after
        // doc_host removes the transcript; the live-row gate must not swallow it.
        let mut tombstone = workspace.acknowledged_chat(id).unwrap();
        tombstone.seq = 10;
        tombstone.deleted = true;
        let _ = registry.apply_state(10, true, 0, vec![tombstone]);
        store.save_snapshot(REGISTRY_DOC_ID, &registry.to_bytes().unwrap()).unwrap();
        store.delete_snapshot(id).unwrap();
        let workspace = open_workspace(&store);
        responses.lock().extend([
            (200, json!({"session":{"revision":8, "sourceVersion":version, "deleted":false}})),
            (200, json!({"revision":9})),
        ]);
        enqueue_registry(&store, &workspace).await.unwrap();
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert!(store.sync_job_version(id, JOB).unwrap().is_none());
        assert!(responses.lock().is_empty());
        version.insert(REGISTRY_VERSION.into(), 10);
        {
            let requests = requests.lock();
            assert_eq!(requests.len(), 8);
            assert_eq!(requests[6].0, "/api/crew-directory/get");
            let deleted = &requests[7];
            assert_eq!(deleted.0, "/api/crew-directory/upsert");
            assert_eq!(deleted.1["sourceVersion"], json!(version));
            assert_eq!(deleted.1["expectedRevision"], 8);
            assert_eq!(deleted.1["deleted"], true);
            assert_eq!(deleted.1["links"], json!([]));
        }
        workspace.shutdown();
        server.abort();
    }

    // Failure modes: a registry reset flag raised from a stale answer (as older
    // builds did) blocks directory publication forever; after a real server
    // reset the flag clears anyway, and rows renumbered below fences this
    // device already sent get published under colliding fences; the cleared
    // flag returns after a restart.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publication_resumes_once_sent_fences_disprove_a_registry_reset() {
        let id = "5b0f3c3e-2d6a-4f3e-9c1d-7e8a9b0c1d2e";
        let doc = loro::LoroDoc::new();
        doc.set_peer_id(42).unwrap();
        let row = doc.get_list("messages").push_container(loro::LoroMap::new()).unwrap();
        row.insert("role", "user").unwrap();
        let parts = row.insert_container("parts", loro::LoroList::new()).unwrap();
        let part = parts.push_container(loro::LoroMap::new()).unwrap();
        part.insert("kind", "text").unwrap();
        part.insert_container("text", loro::LoroText::new()).unwrap()
            .insert(0, "See https://acme.slack.com/archives/C123/p1234567890123456").unwrap();
        doc.commit();
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        let (projected, _) = project(&snapshot).unwrap();
        let fenced = |seq: u64| {
            let mut version = projected.clone();
            version.insert(REGISTRY_VERSION.into(), seq);
            version
        };
        // This device sent, and the directory confirmed, the chat at registry seq 9.
        let sent = serde_json::to_string(&Evidence { source_version: fenced(9), confirmed: true,
            ..Evidence::default() }).unwrap();
        let replies = Replies::new(Mutex::new(VecDeque::from([
            (200, json!({"session":{"revision":7, "sourceVersion":fenced(9), "deleted":false}})),
            (200, json!({"revision":8})),
        ])));
        let requests = Recorded::default();
        let (origin, server) = fake_scaffold(replies.clone(), requests.clone()).await;
        let mut required = BTreeMap::new();

        // A real reset re-acknowledges the chat at 3, below the sent fence: publication stays blocked.
        let dir = tempfile::tempdir().unwrap();
        let (services, store) = signed_in_services(dir.path(), &origin).await;
        store.save_directory_state(id, &sent).unwrap();
        let mut registry = RegistryDoc::new("test-device");
        let _ = registry.apply_state(9, true, 0, vec![chat_row(&services, id, 9)]);
        let _ = registry.apply_state(2, true, 0, Vec::new());
        for batch in registry.take_pushable() {
            assert!(registry.ack_batch(&batch.batch, 3));
        }
        assert!(registry.apply_rows(3, vec![chat_row(&services, id, 3)]));
        store.save_snapshot(REGISTRY_DOC_ID, &registry.to_bytes().unwrap()).unwrap();
        store.save_snapshot(id, &snapshot).unwrap();
        let workspace = open_workspace(&store);
        enqueue_registry(&store, &workspace).await.unwrap();
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert!(workspace.directory_reconciliation_required());
        assert!(requests.lock().is_empty(), "renumbered rows are never published");
        assert!(store.sync_job_version(id, JOB).unwrap().is_some());
        workspace.shutdown();

        // A stale answer raised the flag, as older builds did: nothing was renumbered.
        let dir = tempfile::tempdir().unwrap();
        let (services, store) = signed_in_services(dir.path(), &origin).await;
        store.save_directory_state(id, &sent).unwrap();
        let mut registry = RegistryDoc::new("test-device");
        let _ = registry.apply_state(9, true, 0, vec![chat_row(&services, id, 9)]);
        let _ = registry.apply_state(8, false, 0, Vec::new());
        assert!(registry.directory_reconciliation_required());
        // The chat changes afterwards, so there is something new to publish.
        let _ = registry.apply_state(10, false, 0, vec![chat_row(&services, id, 10)]);
        store.save_snapshot(REGISTRY_DOC_ID, &registry.to_bytes().unwrap()).unwrap();
        store.save_snapshot(id, &snapshot).unwrap();
        let workspace = open_workspace(&store);
        enqueue_registry(&store, &workspace).await.unwrap();
        drain(&store, &services, &workspace, &mut required).await.unwrap();
        assert!(!workspace.directory_reconciliation_required(), "the sent fence disproves the reset");
        assert!(store.sync_job_version(id, JOB).unwrap().is_none(), "publication resumed");
        assert!(replies.lock().is_empty());
        {
            let requests = requests.lock();
            assert_eq!(requests.len(), 2);
            let (path, upsert) = &requests[1];
            assert_eq!(path, "/api/crew-directory/upsert");
            assert_eq!(upsert["expectedRevision"], 7);
            assert_eq!(upsert["sourceVersion"], json!(fenced(10)));
        }
        workspace.shutdown();
        let saved = store.load_snapshot(REGISTRY_DOC_ID).unwrap().unwrap();
        assert!(!RegistryDoc::from_bytes(&saved, "test-device").unwrap().directory_reconciliation_required());
        server.abort();
    }

    #[test]
    fn stale_concurrent_and_reset_sources_never_borrow_server_evidence() {
        let old = BTreeMap::from([(REGISTRY_VERSION.into(), 9), ("s:1".into(), 4)]);
        let mut current = old.clone();
        current.insert(REGISTRY_VERSION.into(), 10);
        assert!(require_observed(&current, &old).is_ok());
        let unseen = BTreeMap::from([(REGISTRY_VERSION.into(), 11), ("s:2".into(), 1)]);
        assert!(require_observed(&current, &unseen).is_err());
        assert!(!current.contains_key("s:2"));
        current.insert(REGISTRY_VERSION.into(), 8);
        assert!(require_observed(&current, &old).unwrap_err().to_string().contains("reconciliation"));
    }

    #[test]
    fn snapshot_projection_scans_chunk_boundaries_and_excludes_private_reasoning() {
        let doc = loro::LoroDoc::new();
        doc.set_peer_id(42).unwrap();
        let row = doc.get_list("messages").push_container(loro::LoroMap::new()).unwrap();
        row.insert("role", "user").unwrap();
        let parts = row.insert_container("parts", loro::LoroList::new()).unwrap();
        let part = parts.push_container(loro::LoroMap::new()).unwrap();
        part.insert("kind", "text").unwrap();
        let text = part.insert_container("text", loro::LoroText::new()).unwrap();
        let url = "https://acme.slack.com/archives/C123/p1234567890123456?thread_ts=1234567890.123456";
        text.insert(0, &format!("{} {url}).", "x".repeat(65530))).unwrap();
        let private = parts.push_container(loro::LoroMap::new()).unwrap();
        private.insert("kind", "reasoning").unwrap();
        private.insert_container("reasoning", loro::LoroText::new()).unwrap()
            .insert(0, "https://notion.so/12345678901234567890123456789012").unwrap();
        doc.commit();
        let bytes = doc.export(loro::ExportMode::Snapshot).unwrap();
        let (version, links) = project(&bytes).unwrap();
        assert_eq!(links, vec![url]);
        assert_eq!(version["s:42"], *doc.oplog_vv().get(&42).unwrap() as u64);
    }

    #[test]
    fn pending_projection_and_observed_fence_survive_restart_and_snapshot_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let id = "9dd173da-fb53-48df-9581-026009c783e2";
        let store = DocsStore::open(dir.path()).unwrap();
        store.save_snapshot(id, b"first").unwrap();
        let first = store.sync_job_version(id, JOB).unwrap().unwrap();
        let evidence = Evidence { source_version: BTreeMap::from([(REGISTRY_VERSION.into(), 12)]), confirmed: false,
            ..Evidence::default() };
        store.save_directory_state(id, &serde_json::to_string(&evidence).unwrap()).unwrap();
        store.save_snapshot(id, b"second").unwrap();
        store.complete_sync_job(id, JOB, first).unwrap();
        assert!(store.sync_job_version(id, JOB).unwrap().is_some());
        store.delete_snapshot(id).unwrap();
        drop(store);
        let store = DocsStore::open(dir.path()).unwrap();
        assert_eq!(store.pending_sync_jobs(JOB, "", 64).unwrap(), vec![id]);
        let saved: Evidence = serde_json::from_str(&store.directory_state(id).unwrap().unwrap()).unwrap();
        assert_eq!(saved.source_version, evidence.source_version);
        assert!(!saved.confirmed, "lost ACK remains retryable, never a new session");
    }
}
