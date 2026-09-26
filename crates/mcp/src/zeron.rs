//! The engine client: a thin, reconnecting wrapper over `zeron_rpc` with the
//! snapshot/resolve helpers the tools share.
//!
//! Watch streams are the engine's only read surface for chats, devices,
//! spaces, sessions, and transcripts — there is no one-shot "get" RPC. A
//! snapshot here is "subscribe, take the first item, drop" (drop cancels
//! server-side), which is exactly what the sidebar does on attach.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use zeron_doc::{
    SessionCommandPayload, SessionMessageEntry, TranscriptFrame, apply_transcript_frame,
};
use zeron_proto::{
    Chat, Device, HarnessId, Model, ReasoningLevel, Session, SessionStatus, Space, SteeringMode,
};
use zeron_rpc::{RpcClient, RpcError, RpcSubscription, connect_ws, methods};

/// First-item wait for a watch snapshot. Localhost; the engine answers
/// watch attaches in milliseconds unless it is still assembling stores.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(15);
/// Gap between resubscribes when a watch stream ends at a lifecycle
/// boundary (chat2 cutover etc.) — the same contract the UI follows.
const RESUBSCRIBE_DELAY: Duration = Duration::from_millis(300);

/// Which chat this server speaks for, when the engine injected it into a
/// harness. Unset when a human runs `zeron mcp` from a terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Origin {
    pub chat_id: Option<String>,
    pub device_id: Option<String>,
}

impl Origin {
    pub fn from_env() -> Self {
        let read = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        Self {
            chat_id: read("ZERON_CHAT_ID"),
            device_id: read("ZERON_DEVICE_ID"),
        }
    }
}

/// `ListHarnesses` row — the engine's `HarnessDescriptor`, re-declared here
/// so this crate does not link the engine.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: HarnessId,
    pub name: String,
    #[serde(default)]
    pub supports_steering: bool,
    #[serde(default = "default_steering_mode")]
    pub steering_mode: SteeringMode,
    #[serde(default)]
    pub reasoning_levels: Vec<ReasoningLevel>,
    #[serde(default = "default_true")]
    pub installed: bool,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn default_true() -> bool {
    true
}

fn default_steering_mode() -> SteeringMode {
    SteeringMode::TurnBoundary
}

impl HarnessInfo {
    /// Offered on this device: installed and not switched off in Settings.
    pub fn available(&self) -> bool {
        self.installed && self.enabled.unwrap_or(true)
    }

    /// Can take a prompt inside the running turn (the composer's "Steer").
    pub fn steers_mid_turn(&self) -> bool {
        self.supports_steering && self.steering_mode == SteeringMode::StepBoundary
    }
}

/// Why a `wait_for_turn` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnOutcome {
    /// The chat finished a turn (or was already idle with nothing pending).
    Completed,
    /// The agent asked a question and is blocked on `respond_to_input`.
    AwaitingInput,
    /// The run failed.
    Errored,
    /// The deadline passed with the chat still working (or never starting).
    TimedOut,
}

pub struct Zeron {
    url: String,
    origin: Origin,
    rpc: Mutex<Option<Arc<RpcClient>>>,
}

impl Zeron {
    /// Lazy dialer: nothing connects until the first tool call, so `zeron
    /// mcp` starts (and answers `initialize`) even before the engine is up.
    pub fn new(url: String, origin: Origin) -> Self {
        Self {
            url,
            origin,
            rpc: Mutex::new(None),
        }
    }

    /// Wrap an already-connected client (tests, in-process embedding).
    pub fn with_client(client: RpcClient, origin: Origin) -> Self {
        Self {
            url: String::new(),
            origin,
            rpc: Mutex::new(Some(Arc::new(client))),
        }
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    async fn client(&self) -> anyhow::Result<Arc<RpcClient>> {
        let mut slot = self.rpc.lock().await;
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        if self.url.is_empty() {
            bail!("engine connection closed");
        }
        let client = connect_ws(&self.url).await.map_err(|e| {
            anyhow!(
                "no Zeron engine listening at {} ({e}) — is Zeron running?",
                self.url
            )
        })?;
        let client = Arc::new(client);
        *slot = Some(client.clone());
        Ok(client)
    }

    async fn forget_client(&self) {
        self.rpc.lock().await.take();
    }

    /// Unary call with one reconnect on a closed socket (the engine
    /// restarted underneath a long-lived agent session).
    pub async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let client = self.client().await?;
        match client.call(method, params.clone()).await {
            Err(RpcError::Closed) => {
                self.forget_client().await;
                let client = self.client().await?;
                client
                    .call(method, params)
                    .await
                    .map_err(|e| anyhow!("{method}: {e}"))
            }
            other => other.map_err(|e| anyhow!("{method}: {e}")),
        }
    }

    /// Open a watch stream (reconnecting once if the socket is gone).
    pub async fn subscribe(&self, method: &str, params: Value) -> anyhow::Result<RpcSubscription> {
        let client = self.client().await?;
        match client.subscribe_scoped(method, params.clone()).await {
            Err(RpcError::Closed) => {
                self.forget_client().await;
                let client = self.client().await?;
                client
                    .subscribe_scoped(method, params)
                    .await
                    .map_err(|e| anyhow!("{method}: {e}"))
            }
            other => other.map_err(|e| anyhow!("{method}: {e}")),
        }
    }

    /// First item of a watch stream, then cancel it.
    pub async fn snapshot(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let mut rx = self.subscribe(method, params).await?;
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, rx.recv()).await {
            Ok(Some(item)) => Ok(item),
            Ok(None) => bail!("{method}: stream ended before its first snapshot"),
            Err(_) => bail!(
                "{method}: no snapshot within {}s",
                SNAPSHOT_TIMEOUT.as_secs()
            ),
        }
    }

    async fn snapshot_as<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> anyhow::Result<T> {
        let value = self.snapshot(method, params).await?;
        serde_json::from_value(value).with_context(|| format!("{method}: unexpected shape"))
    }

    // ---- reads -------------------------------------------------------------

    pub async fn local_device_id(&self) -> anyhow::Result<String> {
        let value = self.call(methods::LOCAL_DEVICE, json!({})).await?;
        value
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("LocalDevice reply missing deviceId"))
    }

    pub async fn engine_info(&self) -> anyhow::Result<Value> {
        self.call(methods::ENGINE_INFO, json!({})).await
    }

    pub async fn devices(&self) -> anyhow::Result<Vec<Device>> {
        self.snapshot_as(methods::WATCH_DEVICES, json!({})).await
    }

    pub async fn spaces(&self) -> anyhow::Result<Vec<Space>> {
        self.snapshot_as(methods::WATCH_SPACES, json!({})).await
    }

    pub async fn chats(&self) -> anyhow::Result<Vec<Chat>> {
        self.snapshot_as(methods::WATCH_CHATS, json!({})).await
    }

    pub async fn sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.snapshot_as(methods::WATCH_SESSIONS, json!({})).await
    }

    pub async fn harnesses(&self) -> anyhow::Result<Vec<HarnessInfo>> {
        let value = self.call(methods::LIST_HARNESSES, json!({})).await?;
        serde_json::from_value(value).context("ListHarnesses: unexpected shape")
    }

    pub async fn models(&self, harness: HarnessId) -> anyhow::Result<Vec<Model>> {
        let value = self
            .call(methods::LIST_MODELS, json!({ "harness": harness }))
            .await?;
        serde_json::from_value(value).context("ListModels: unexpected shape")
    }

    /// The full transcript: attach, take the opening `reset` frame, detach.
    pub async fn transcript(&self, chat_id: &str) -> anyhow::Result<Vec<SessionMessageEntry>> {
        let mut rx = self
            .subscribe(methods::WATCH_DOC_MESSAGES, json!({ "chatId": chat_id }))
            .await?;
        let mut entries = Vec::new();
        let deadline = Instant::now() + SNAPSHOT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "WatchDocMessages: no transcript within {}s",
                    SNAPSHOT_TIMEOUT.as_secs()
                );
            }
            let item = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(item)) => item,
                Ok(None) => bail!("WatchDocMessages: stream ended before the transcript arrived"),
                Err(_) => bail!(
                    "WatchDocMessages: no transcript within {}s",
                    SNAPSHOT_TIMEOUT.as_secs()
                ),
            };
            let is_reset = item.get("reset").is_some();
            let frame: TranscriptFrame =
                serde_json::from_value(item).context("WatchDocMessages: bad frame")?;
            apply_transcript_frame(&mut entries, frame).map_err(|e| anyhow!("transcript: {e}"))?;
            if is_reset {
                return Ok(entries);
            }
        }
    }

    // ---- writes ------------------------------------------------------------

    pub async fn mutate(&self, params: Value) -> anyhow::Result<Value> {
        self.call(methods::MUTATE, params).await
    }

    /// Durable command into the chat doc; the host device drains it.
    pub async fn queue_command(
        &self,
        chat_id: &str,
        payload: &SessionCommandPayload,
    ) -> anyhow::Result<String> {
        let command = serde_json::to_value(payload).context("serialize command")?;
        let reply = self
            .call(
                methods::QUEUE_COMMAND,
                json!({ "chatId": chat_id, "command": command }),
            )
            .await?;
        Ok(reply
            .get("commandId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// Queue-row send for a busy chat whose harness cannot steer mid-turn:
    /// the host promotes it when the live turn ends.
    pub async fn queue_message(&self, chat_id: &str, text: &str) -> anyhow::Result<String> {
        let reply = self
            .call(
                methods::QUEUE_MESSAGE,
                json!({ "chatId": chat_id, "text": text, "holdForTurnEnd": true }),
            )
            .await?;
        Ok(reply
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    // ---- resolution --------------------------------------------------------

    /// Full id, unique id prefix, or exact (case-insensitive) title.
    pub async fn resolve_chat(&self, key: &str) -> anyhow::Result<Chat> {
        let key = key.trim();
        if key.is_empty() {
            bail!("chat is required");
        }
        let chats = self.chats().await?;
        resolve_chat_in(&chats, key)
    }

    /// Space id, exact path, display name, or unique path suffix.
    pub async fn resolve_space(&self, key: &str) -> anyhow::Result<Space> {
        let spaces = self.spaces().await?;
        resolve_space_in(&spaces, key.trim())
    }

    /// Device id or exact name; `None` means this engine's own device.
    pub async fn resolve_device_id(&self, key: Option<&str>) -> anyhow::Result<String> {
        let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) else {
            return self.local_device_id().await;
        };
        let devices = self.devices().await?;
        if let Some(device) = devices.iter().find(|d| d.id == key) {
            return Ok(device.id.clone());
        }
        let by_name: Vec<&Device> = devices
            .iter()
            .filter(|d| d.name.eq_ignore_ascii_case(key))
            .collect();
        match by_name.as_slice() {
            [one] => Ok(one.id.clone()),
            [] => bail!(
                "no device matches {key:?}; known: {}",
                devices
                    .iter()
                    .map(|d| format!("{} ({})", d.name, d.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            many => bail!(
                "{} devices are named {key:?}; use an id: {}",
                many.len(),
                many.iter()
                    .map(|d| d.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    // ---- turn waiting ------------------------------------------------------

    /// Block until `chat`'s turn settles.
    ///
    /// `expect_turn` says a send is in flight: the wait then ends only when
    /// the session row moves past `baseline` (a new `last_completed_turn`,
    /// an `awaitingInput`/`errored` stamp newer than the baseline, or a
    /// working→idle edge) — a chat with no row yet, or an idle row identical
    /// to the baseline, is a run that has not started and keeps waiting.
    /// Without `expect_turn` the current posture is answered immediately
    /// unless the chat is working.
    pub async fn wait_for_turn(
        &self,
        chat: &Chat,
        baseline: Option<&Session>,
        expect_turn: bool,
        timeout: Duration,
    ) -> anyhow::Result<(TurnOutcome, Option<Session>)> {
        let deadline = Instant::now() + timeout;
        let mut saw_working = false;
        let mut last: Option<Session> = baseline.cloned();
        'resubscribe: loop {
            if deadline.saturating_duration_since(Instant::now()).is_zero() {
                return Ok((TurnOutcome::TimedOut, last));
            }
            let mut rx = self.subscribe(methods::WATCH_SESSIONS, json!({})).await?;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok((TurnOutcome::TimedOut, last));
                }
                let item = match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Some(item)) => item,
                    Ok(None) => {
                        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                        continue 'resubscribe;
                    }
                    Err(_) => return Ok((TurnOutcome::TimedOut, last)),
                };
                let sessions: Vec<Session> = match serde_json::from_value(item) {
                    Ok(sessions) => sessions,
                    Err(err) => {
                        tracing::warn!(error = %err, "WatchSessions: unexpected item");
                        continue;
                    }
                };
                let Some(session) = session_for(&sessions, chat) else {
                    // No row at all: the chat never ran. With a send in
                    // flight the row appears when the host picks it up.
                    if expect_turn {
                        continue;
                    }
                    return Ok((TurnOutcome::Completed, None));
                };
                let advanced = baseline.is_none_or(|b| {
                    session.updated_at > b.updated_at
                        || session.last_completed_turn != b.last_completed_turn
                });
                match session.status {
                    SessionStatus::Working => {
                        saw_working = true;
                        last = Some(session);
                    }
                    SessionStatus::AwaitingInput if advanced || saw_working => {
                        return Ok((TurnOutcome::AwaitingInput, Some(session)));
                    }
                    SessionStatus::Errored if advanced || saw_working => {
                        return Ok((TurnOutcome::Errored, Some(session)));
                    }
                    SessionStatus::Idle => {
                        let turn_changed = match baseline {
                            Some(b) => b.last_completed_turn != session.last_completed_turn,
                            // A row that did not exist at send time only
                            // counts once it records a finished turn.
                            None if expect_turn => session.last_completed_turn.is_some(),
                            None => true,
                        };
                        if turn_changed || saw_working {
                            return Ok((TurnOutcome::Completed, Some(session)));
                        }
                        last = Some(session);
                    }
                    _ => {
                        last = Some(session);
                    }
                }
            }
        }
    }
}

/// The session row that speaks for `chat`: the host device's, else the
/// freshest one (a chat re-homed mid-flight can briefly have two).
pub fn session_for(sessions: &[Session], chat: &Chat) -> Option<Session> {
    let mine: Vec<&Session> = sessions.iter().filter(|s| s.chat_id == chat.id).collect();
    mine.iter()
        .find(|s| s.device_id == chat.device_id)
        .or_else(|| mine.iter().max_by_key(|s| s.updated_at))
        .map(|s| (*s).clone())
}

pub fn resolve_chat_in(chats: &[Chat], key: &str) -> anyhow::Result<Chat> {
    if let Some(chat) = chats.iter().find(|c| c.id == key) {
        return Ok(chat.clone());
    }
    let by_prefix: Vec<&Chat> = chats.iter().filter(|c| c.id.starts_with(key)).collect();
    if let [one] = by_prefix.as_slice() {
        return Ok((*one).clone());
    }
    let by_title: Vec<&Chat> = chats
        .iter()
        .filter(|c| {
            c.title
                .as_deref()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case(key))
        })
        .collect();
    if let [one] = by_title.as_slice() {
        return Ok((*one).clone());
    }
    if by_prefix.len() > 1 {
        bail!(
            "{} chats start with {key:?}; be more specific: {}",
            by_prefix.len(),
            by_prefix
                .iter()
                .map(|c| short(&c.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if by_title.len() > 1 {
        bail!(
            "{} chats are titled {key:?}; use an id: {}",
            by_title.len(),
            by_title
                .iter()
                .map(|c| short(&c.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    bail!("no chat matches {key:?} (id, id prefix, or exact title); try list_chats")
}

pub fn resolve_space_in(spaces: &[Space], key: &str) -> anyhow::Result<Space> {
    if key.is_empty() {
        bail!("project is required");
    }
    if let Some(space) = spaces.iter().find(|s| s.id == key || s.path == key) {
        return Ok(space.clone());
    }
    let normalized = key.trim_end_matches(['/', '\\']);
    let by_name: Vec<&Space> = spaces
        .iter()
        .filter(|s| s.display_name().eq_ignore_ascii_case(normalized))
        .collect();
    if let [one] = by_name.as_slice() {
        return Ok((*one).clone());
    }
    let by_suffix: Vec<&Space> = spaces
        .iter()
        .filter(|s| {
            let path = s.path.trim_end_matches(['/', '\\']);
            path.ends_with(normalized) || path.contains(normalized)
        })
        .collect();
    if let [one] = by_suffix.as_slice() {
        return Ok((*one).clone());
    }
    let candidates = if by_name.len() > 1 {
        &by_name
    } else {
        &by_suffix
    };
    if candidates.len() > 1 {
        bail!(
            "{} projects match {key:?}; use an id: {}",
            candidates.len(),
            candidates
                .iter()
                .map(|s| format!("{} ({})", s.path, s.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    bail!("no project matches {key:?} (id, path, or name); try list_projects")
}

/// Eight-char id prefix, the sidebar's convention.
pub fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn one_shot_reads_release_quiet_watches() {
        use futures::StreamExt;
        struct Service(tokio::sync::watch::Sender<()>);
        #[async_trait::async_trait]
        impl zeron_rpc::RpcService for Service {
            async fn handle(
                &self,
                method: &str,
                _: Value,
            ) -> Result<zeron_rpc::RpcReply, RpcError> {
                let first = if method == methods::WATCH_DOC_MESSAGES {
                    json!({"reset":[]})
                } else {
                    json!([])
                };
                let rx = self.0.subscribe();
                Ok(zeron_rpc::RpcReply::Stream(
                    futures::stream::unfold((Some(first), rx), |(first, mut rx)| async move {
                        if let Some(item) = first {
                            return Some((item, (None, rx)));
                        }
                        rx.changed().await.ok()?;
                        Some((json!([]), (None, rx)))
                    })
                    .boxed(),
                ))
            }
        }
        let (watched, _) = tokio::sync::watch::channel(());
        let rpc = zeron_rpc::memory_client(Arc::new(Service(watched.clone())));
        let client = Zeron::with_client(rpc, Origin::default());
        for _ in 0..16 {
            assert!(client.transcript("quiet").await.unwrap().is_empty());
            client
                .snapshot(methods::WATCH_CHATS, json!({}))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while watched.receiver_count() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("one-shot watch cancelled without another event");
        }
        // Keep the RPC connection alive throughout: disconnect must not be the cleanup.
        client
            .snapshot(methods::WATCH_DEVICES, json!({}))
            .await
            .unwrap();
    }

    fn chat(id: &str, title: Option<&str>) -> Chat {
        Chat {
            id: id.into(),
            device_id: "dev".into(),
            title: title.map(str::to_owned),
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            parent_chat_id: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn chat_resolution_prefers_id_then_prefix_then_title() {
        let chats = vec![
            chat("aaaa1111-1", Some("Fix login")),
            chat("aaaa2222-2", Some("Fix Login")),
            chat("bbbb3333-3", Some("Docs")),
        ];
        assert_eq!(
            resolve_chat_in(&chats, "bbbb3333-3").unwrap().id,
            "bbbb3333-3"
        );
        assert_eq!(resolve_chat_in(&chats, "bbbb").unwrap().id, "bbbb3333-3");
        assert_eq!(resolve_chat_in(&chats, "docs").unwrap().id, "bbbb3333-3");
        let err = resolve_chat_in(&chats, "aaaa").unwrap_err().to_string();
        assert!(err.contains("2 chats start with"), "{err}");
        let err = resolve_chat_in(&chats, "fix login")
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 chats are titled"), "{err}");
        assert!(resolve_chat_in(&chats, "zzz").is_err());
    }

    fn space(id: &str, path: &str, name: Option<&str>) -> Space {
        Space {
            id: id.into(),
            device_id: "dev".into(),
            path: path.into(),
            name: name.map(str::to_owned),
            git_detected: true,
            git_checked_at: None,
            checkout_id: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn space_resolution_accepts_id_path_name_or_suffix() {
        let spaces = vec![
            space("s1", "/home/u/GitHub/comet", None),
            space("s2", "/home/u/GitHub/comet-ios", Some("iOS")),
        ];
        assert_eq!(resolve_space_in(&spaces, "s2").unwrap().id, "s2");
        assert_eq!(
            resolve_space_in(&spaces, "/home/u/GitHub/comet")
                .unwrap()
                .id,
            "s1"
        );
        assert_eq!(resolve_space_in(&spaces, "ios").unwrap().id, "s2");
        assert_eq!(resolve_space_in(&spaces, "comet-ios").unwrap().id, "s2");
        // "comet" is the exact display name of s1 even though it is also a
        // substring of s2's path.
        assert_eq!(resolve_space_in(&spaces, "comet").unwrap().id, "s1");
        assert!(resolve_space_in(&spaces, "GitHub").is_err());
    }
}
