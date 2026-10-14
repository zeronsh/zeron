//! Turn-end notices: when a chat finishes the turn that handled a message
//! another chat sent it, tell the sender — without the sender waiting.
//!
//! This is what lets an orchestrating agent delegate and move on, the way a
//! background subagent works: the send registers a [`Watch`], and when the
//! receiving chat's turn settles the sender's agent gets the final response
//! as a background event — the way its own background work wakes it, with
//! nothing written to the sender's transcript or queue. An idle sender starts
//! a turn on it, a running turn takes it at its next step where the harness
//! allows, and otherwise it waits for the turn to end (a notice never
//! interrupts work). Notices that land close together share one event.
//!
//! The engine runs this (not the per-agent `zeron mcp` process, which lives
//! and dies with one harness), and keeps the watches on disk so a restart
//! picks them back up: completion is read from the transcript, so a turn
//! that ended while the engine was down still reports.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_proto::Chat;
use zeron_rpc::methods;

use crate::zeron::{MessageTurn, TurnOutcome, Zeron, short};

/// A watch that never settles (a chat left running, a host gone for good)
/// is dropped after this long.
const WATCH_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);
/// Notices that land within this window of each other share one message.
const BATCH_WINDOW: Duration = Duration::from_millis(1500);
/// Retry gap for transient failures (engine still assembling, link down).
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// A final response longer than this is clipped, with a pointer to read_chat.
const MAX_REPLY_CHARS: usize = 8000;

pub use zeron_proto::agent_messages::{NOTICE_FOOTER, NOTICE_PREFIX};

/// One pending "tell `watcher` when `chat` finishes the turn that handles
/// `message_id`".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    pub watcher_chat_id: String,
    pub chat_id: String,
    /// The user message whose turn is watched.
    pub message_id: String,
    pub created_at: DateTime<Utc>,
    /// A question already reported; the watch keeps going past it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_input: Option<String>,
}

impl Watch {
    pub fn new(watcher_chat_id: &str, chat_id: &str, message_id: &str) -> Self {
        Self {
            watcher_chat_id: watcher_chat_id.to_owned(),
            chat_id: chat_id.to_owned(),
            message_id: message_id.to_owned(),
            created_at: Utc::now(),
            reported_input: None,
        }
    }

    fn key(&self) -> String {
        key(&self.watcher_chat_id, &self.chat_id)
    }
}

fn key(watcher: &str, chat: &str) -> String {
    format!("{watcher}\u{1f}{chat}")
}

#[derive(Clone)]
pub struct TurnNotices {
    inner: Arc<Inner>,
}

struct Inner {
    path: Option<PathBuf>,
    zeron: OnceLock<Arc<Zeron>>,
    batch_window: Duration,
    watches: Mutex<HashMap<String, Live>>,
    /// Per watcher: sections waiting for the batch window to close.
    outboxes: Mutex<HashMap<String, Vec<String>>>,
}

struct Live {
    watch: Watch,
    task: Option<tokio::task::AbortHandle>,
}

impl TurnNotices {
    /// Load persisted watches from `path` (none when `None`). Nothing runs
    /// until [`Self::start`].
    pub fn new(path: Option<PathBuf>) -> Self {
        let watches = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<Watch>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|watch| (watch.key(), Live { watch, task: None }))
            .collect();
        Self {
            inner: Arc::new(Inner {
                path,
                zeron: OnceLock::new(),
                batch_window: BATCH_WINDOW,
                watches: Mutex::new(watches),
                outboxes: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Like [`Self::new`], with a shorter batch window for tests.
    pub fn with_batch_window(path: Option<PathBuf>, window: Duration) -> Self {
        let mut notices = Self::new(path);
        Arc::get_mut(&mut notices.inner)
            .expect("fresh notices are unshared")
            .batch_window = window;
        notices
    }

    /// Begin watching through `zeron` (the engine's own RPC surface), resuming
    /// every persisted watch. A second call is a no-op.
    pub fn start(&self, zeron: Arc<Zeron>) {
        if self.inner.zeron.set(zeron).is_err() {
            return;
        }
        let keys: Vec<String> = lock(&self.inner.watches).keys().cloned().collect();
        for key in keys {
            self.spawn(&key);
        }
    }

    pub fn started(&self) -> bool {
        self.inner.zeron.get().is_some()
    }

    /// Watch a turn. A newer send to the same chat replaces the older watch:
    /// one notice per chat, for the latest thing it was asked.
    pub fn register(&self, watch: Watch) {
        let key = watch.key();
        {
            let mut watches = lock(&self.inner.watches);
            if let Some(old) = watches.insert(key.clone(), Live { watch, task: None })
                && let Some(task) = old.task
            {
                task.abort();
            }
        }
        self.persist();
        self.spawn(&key);
    }

    /// Stop watching `chat` for `watcher` (its result was read some other
    /// way, or the watcher stopped the chat itself). `true` when one existed.
    pub fn cancel(&self, watcher: &str, chat: &str) -> bool {
        let removed = lock(&self.inner.watches).remove(&key(watcher, chat));
        let Some(removed) = removed else {
            return false;
        };
        if let Some(task) = removed.task {
            task.abort();
        }
        self.persist();
        true
    }

    /// Pending watches, optionally only `watcher`'s, oldest first.
    pub fn list(&self, watcher: Option<&str>) -> Vec<Watch> {
        let mut watches: Vec<Watch> = lock(&self.inner.watches)
            .values()
            .filter(|live| watcher.is_none_or(|w| live.watch.watcher_chat_id == w))
            .map(|live| live.watch.clone())
            .collect();
        watches.sort_by_key(|watch| watch.created_at);
        watches
    }

    /// Abort every watch task (they resume from disk on the next start).
    pub fn shutdown(&self) {
        for live in lock(&self.inner.watches).values_mut() {
            if let Some(task) = live.task.take() {
                task.abort();
            }
        }
    }

    fn spawn(&self, key: &str) {
        let Some(zeron) = self.inner.zeron.get().cloned() else {
            return;
        };
        let notices = self.clone();
        let task_key = key.to_owned();
        let mut watches = lock(&self.inner.watches);
        let Some(live) = watches.get_mut(key) else {
            return;
        };
        let task = tokio::spawn(async move { notices.run(zeron, task_key).await });
        if let Some(old) = live.task.replace(task.abort_handle()) {
            old.abort();
        }
    }

    fn current(&self, key: &str) -> Option<Watch> {
        lock(&self.inner.watches)
            .get(key)
            .map(|live| live.watch.clone())
    }

    /// Drop the watch at `key` if it is still `watch` (a newer send may
    /// have replaced it while this task was finishing).
    fn finish(&self, key: &str, watch: &Watch) {
        let mut watches = lock(&self.inner.watches);
        if watches.get(key).is_some_and(|live| live.watch == *watch) {
            watches.remove(key);
            drop(watches);
            self.persist();
        }
    }

    async fn run(self, zeron: Arc<Zeron>, key: String) {
        loop {
            let Some(watch) = self.current(&key) else {
                return;
            };
            let age = (Utc::now() - watch.created_at).to_std().unwrap_or_default();
            let Some(remaining) = WATCH_EXPIRY.checked_sub(age) else {
                tracing::info!(watcher = %watch.watcher_chat_id, chat = %watch.chat_id,
                    "turn notice expired");
                self.finish(&key, &watch);
                return;
            };
            let chat = match zeron.resolve_chat(&watch.chat_id).await {
                Ok(chat) if chat.id == watch.chat_id => chat,
                Ok(_) => {
                    self.finish(&key, &watch);
                    return;
                }
                Err(err) => {
                    let gone = err.to_string().starts_with("no chat matches");
                    tracing::debug!(chat = %watch.chat_id, error = %err, "turn notice: chat lookup failed");
                    if gone {
                        self.finish(&key, &watch);
                        return;
                    }
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            };
            let turn = match zeron
                .await_message_turn(
                    &chat,
                    &watch.message_id,
                    remaining,
                    watch.reported_input.as_deref(),
                )
                .await
            {
                Ok(turn) => turn,
                Err(err) => {
                    tracing::debug!(chat = %watch.chat_id, error = %err, "turn notice: wait failed");
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            };
            match turn.outcome {
                // Loop: the expiry check above ends it.
                TurnOutcome::TimedOut => continue,
                TurnOutcome::AwaitingInput => {
                    let request = turn.pending_input().and_then(|p| {
                        p.get("requestId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                    // Re-check the watch is still ours before reporting.
                    if self.current(&key).as_ref() != Some(&watch) {
                        return;
                    }
                    self.deliver(&zeron, &watch.watcher_chat_id, section(&chat, &turn));
                    let mut watches = lock(&self.inner.watches);
                    if let Some(live) = watches.get_mut(&key)
                        && live.watch == watch
                    {
                        live.watch.reported_input = request;
                    }
                    drop(watches);
                    self.persist();
                }
                TurnOutcome::Completed | TurnOutcome::Errored => {
                    if self.current(&key).as_ref() != Some(&watch) {
                        return;
                    }
                    self.finish(&key, &watch);
                    self.deliver(&zeron, &watch.watcher_chat_id, section(&chat, &turn));
                    return;
                }
            }
        }
    }

    /// Add a section to `watcher`'s next notice; the first one in a window
    /// schedules the flush.
    fn deliver(&self, zeron: &Arc<Zeron>, watcher: &str, section: String) {
        let first = {
            let mut outboxes = lock(&self.inner.outboxes);
            let pending = outboxes.entry(watcher.to_owned()).or_default();
            pending.push(section);
            pending.len() == 1
        };
        if !first {
            return;
        }
        let notices = self.clone();
        let zeron = zeron.clone();
        let watcher = watcher.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(notices.inner.batch_window).await;
            notices.flush(&zeron, &watcher).await;
        });
    }

    /// Send the batch to `watcher`'s agent as one background event.
    async fn flush(&self, zeron: &Zeron, watcher: &str) {
        let pending = lock(&self.inner.outboxes)
            .remove(watcher)
            .unwrap_or_default();
        if pending.is_empty() {
            return;
        }
        let text = compose(&pending);
        for attempt in 0..3 {
            match zeron
                .call(
                    methods::DELIVER_EVENT,
                    json!({ "chatId": watcher, "text": text }),
                )
                .await
            {
                Ok(_) => return,
                Err(err) => {
                    tracing::warn!(watcher = %watcher, attempt, error = %err,
                        "turn notice delivery failed");
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }

    fn persist(&self) {
        let Some(path) = self.inner.path.as_deref() else {
            return;
        };
        let watches = self.list(None);
        let Ok(bytes) = serde_json::to_vec_pretty(&watches) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
            tracing::warn!(path = %path.display(), error = %err, "turn notices: persist failed");
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One notice message: its sections, then the footer once.
pub fn compose(sections: &[String]) -> String {
    let mut text = sections.join("\n\n");
    text.push_str("\n\n");
    text.push_str(NOTICE_FOOTER);
    text
}

/// `"Title" (id)`, or just the id for an untitled chat.
pub fn chat_label(chat: &Chat) -> String {
    zeron_proto::agent_messages::notice_label(chat.title.as_deref(), &chat.id)
}

/// What one settled turn says to the chat that asked for it.
pub fn section(chat: &Chat, turn: &MessageTurn) -> String {
    let label = chat_label(chat);
    let replies = turn.replies();
    let last_reply = replies
        .iter()
        .rev()
        .map(|m| m.text.trim())
        .find(|text| !text.is_empty());
    match turn.outcome {
        TurnOutcome::AwaitingInput => {
            let mut text = format!(
                "{NOTICE_PREFIX}{label} is waiting for an answer. Answer it with respond_to_input.]"
            );
            if let Some(reply) = last_reply {
                text.push_str("\n\n");
                text.push_str(&clip_reply(reply, &chat.id));
            }
            if let Some(input) = turn.pending_input() {
                text.push_str("\n\n");
                text.push_str(&describe_input(&input));
            }
            text
        }
        TurnOutcome::Errored => {
            let errors: Vec<&str> = turn
                .after
                .iter()
                .flat_map(|m| m.errors.iter().map(String::as_str))
                .collect();
            let mut text = format!("{NOTICE_PREFIX}{label} stopped with an error.]");
            if !errors.is_empty() {
                text.push_str("\n\n");
                text.push_str(&errors.join("\n"));
            }
            if let Some(reply) = last_reply {
                text.push_str("\n\n");
                text.push_str(&clip_reply(reply, &chat.id));
            }
            text
        }
        TurnOutcome::Completed | TurnOutcome::TimedOut => {
            let mut body = match last_reply {
                Some(reply) => clip_reply(reply, &chat.id),
                None => "(It ended the turn without a written response.)".to_owned(),
            };
            if turn.background > 0 {
                body.push_str(&format!(
                    "\n\n(It still has {} background task{} running.)",
                    turn.background,
                    if turn.background == 1 { "" } else { "s" }
                ));
            }
            format!("{NOTICE_PREFIX}{label} finished its turn.]\n\n{body}")
        }
    }
}

fn clip_reply(reply: &str, chat_id: &str) -> String {
    if reply.chars().count() <= MAX_REPLY_CHARS {
        return reply.to_owned();
    }
    let clipped: String = reply.chars().take(MAX_REPLY_CHARS).collect();
    format!(
        "{clipped}…\n\n(Response clipped; read_chat {} for the rest.)",
        short(chat_id)
    )
}

/// The pending question in a few readable lines, with its request id.
fn describe_input(input: &Value) -> String {
    let mut lines = Vec::new();
    for question in input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |name: &str| question.get(name).and_then(Value::as_str).unwrap_or("");
        let mut line = format!("- {} (question_id {})", field("question"), field("id"));
        let options: Vec<&str> = question
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !options.is_empty() {
            line.push_str(&format!(": {}", options.join(" | ")));
        }
        lines.push(line);
    }
    if let Some(request) = input.get("requestId").and_then(Value::as_str) {
        lines.push(format!("request_id {request}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::StreamExt;
    use tokio_stream_compat::watch_stream;
    use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client};

    /// A tiny engine: chats, live session rows, transcripts, and each chat's
    /// queue. Session/transcript changes publish to open watches.
    struct Fake {
        state: tokio::sync::watch::Sender<State>,
        /// QueueMessage / UpdateQueuedMessage calls, in order.
        writes: Mutex<Vec<(String, Value)>>,
    }

    #[derive(Clone, Default)]
    struct State {
        sessions: Vec<Value>,
        transcripts: HashMap<String, Vec<Value>>,
        queues: HashMap<String, Vec<(String, String)>>,
    }

    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: tokio::sync::watch::channel(State::default()).0,
                writes: Mutex::new(Vec::new()),
            })
        }

        fn status(&self, chat: &str, status: &str) {
            self.status_with_background(chat, status, 0);
        }

        fn status_with_background(&self, chat: &str, status: &str, background: u32) {
            self.state.send_modify(|state| {
                state.sessions.retain(|s| s["chatId"] != chat);
                state.sessions.push(json!({
                    "chatId": chat, "deviceId": "dev", "status": status,
                    "updatedAt": Utc::now(), "lastCompletedTurn": null,
                    "backgroundTasks": background,
                }));
            });
        }

        fn message(&self, chat: &str, id: &str, role: &str, text: &str, streaming: bool) {
            self.state.send_modify(|state| {
                let entries = state.transcripts.entry(chat.to_owned()).or_default();
                entries.retain(|e| e["id"] != id);
                entries.push(json!({
                    "id": id, "role": role, "createdAt": 1, "deviceId": "dev",
                    "status": if streaming { "streaming" } else { "complete" },
                    "parts": [{ "kind": "text", "id": "t", "text": text }],
                }));
            });
        }

        /// The watcher drains its queue (its turn started).
        fn drain(&self, chat: &str) -> Vec<String> {
            let mut taken = Vec::new();
            self.state.send_modify(|state| {
                taken = state
                    .queues
                    .remove(chat)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, text)| text)
                    .collect();
            });
            taken
        }

        fn queued(&self, chat: &str) -> Vec<String> {
            self.state
                .borrow()
                .queues
                .get(chat)
                .map(|rows| rows.iter().map(|(_, text)| text.clone()).collect())
                .unwrap_or_default()
        }
    }

    mod tokio_stream_compat {
        use futures::stream::BoxStream;
        use serde_json::Value;

        pub fn watch_stream<T: Clone + Send + Sync + 'static>(
            rx: tokio::sync::watch::Receiver<T>,
            project: fn(&T) -> Value,
        ) -> BoxStream<'static, Value> {
            Box::pin(futures::stream::unfold(
                (rx, true),
                move |(mut rx, first)| async move {
                    if !first {
                        rx.changed().await.ok()?;
                    }
                    let value = project(&rx.borrow_and_update());
                    Some((value, (rx, false)))
                },
            ))
        }
    }

    #[async_trait]
    impl RpcService for Fake {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            Ok(match method {
                methods::WATCH_CHATS => RpcReply::Stream(
                    futures::stream::iter(vec![json!([
                        { "id": "parent", "deviceId": "dev", "title": "Lead", "archived": false,
                          "createdAt": "2026-10-01T00:00:00Z" },
                        { "id": "child-a", "deviceId": "dev", "title": "Fix \"tests\"", "archived": false,
                          "parentChatId": "parent", "createdAt": "2026-10-01T00:00:00Z" },
                        { "id": "child-b", "deviceId": "dev", "archived": false,
                          "parentChatId": "parent", "createdAt": "2026-10-01T00:00:00Z" },
                    ])])
                    .boxed(),
                ),
                methods::WATCH_SESSIONS => RpcReply::Stream(watch_stream(
                    self.state.subscribe(),
                    |state| Value::Array(state.sessions.clone()),
                )),
                methods::WATCH_DOC_MESSAGES => {
                    let chat = params["chatId"].as_str().unwrap_or_default().to_owned();
                    let entries = self
                        .state
                        .borrow()
                        .transcripts
                        .get(&chat)
                        .cloned()
                        .unwrap_or_default();
                    RpcReply::Stream(futures::stream::iter(vec![json!({ "reset": entries })]).boxed())
                }
                // Background events land in the fake's per-chat inbox.
                methods::DELIVER_EVENT | methods::QUEUE_MESSAGE => {
                    self.writes.lock().unwrap().push((method.into(), params.clone()));
                    let chat = params["chatId"].as_str().unwrap().to_owned();
                    let id = format!("row-{}", self.writes.lock().unwrap().len());
                    let text = params["text"].as_str().unwrap().to_owned();
                    let row = id.clone();
                    self.state.send_modify(|state| {
                        state.queues.entry(chat).or_default().push((row, text));
                    });
                    RpcReply::Value(json!({ "id": id }))
                }
                methods::UPDATE_QUEUED_MESSAGE => {
                    self.writes.lock().unwrap().push((method.into(), params.clone()));
                    let chat = params["chatId"].as_str().unwrap().to_owned();
                    let id = params["id"].as_str().unwrap().to_owned();
                    let text = params["text"].as_str().unwrap().to_owned();
                    let mut changed = false;
                    self.state.send_modify(|state| {
                        if let Some(row) = state
                            .queues
                            .get_mut(&chat)
                            .and_then(|rows| rows.iter_mut().find(|(row, _)| *row == id))
                        {
                            row.1 = text;
                            changed = true;
                        }
                    });
                    RpcReply::Value(json!({ "changed": changed }))
                }
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn started(fake: &Arc<Fake>, path: Option<PathBuf>) -> TurnNotices {
        let notices = TurnNotices::with_batch_window(path, Duration::from_millis(60));
        notices.start(Arc::new(Zeron::with_client(
            memory_client(fake.clone()),
            crate::Origin::default(),
        )));
        notices
    }

    async fn eventually(mut check: impl FnMut() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("condition not met in time");
    }

    #[tokio::test]
    async fn a_finished_turn_queues_its_final_response_for_the_watcher() {
        let fake = Fake::new();
        let notices = started(&fake, None);
        fake.message("child-a", "m1", "user", "go", false);
        fake.status("child-a", "working");
        notices.register(Watch::new("parent", "child-a", "m1"));
        fake.message("child-a", "r1", "assistant", "halfway", true);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            fake.queued("parent").is_empty(),
            "nothing until the turn ends"
        );

        fake.message(
            "child-a",
            "r1",
            "assistant",
            "All 12 tests pass now.",
            false,
        );
        fake.status("child-a", "idle");
        eventually(|| !fake.queued("parent").is_empty()).await;
        let queued = fake.queued("parent");
        assert_eq!(queued.len(), 1);
        let text = &queued[0];
        assert!(
            text.starts_with("[Zeron notice: chat \"Fix 'tests'\" (child-a) finished its turn.]"),
            "{text}"
        );
        assert!(text.contains("All 12 tests pass now."), "{text}");
        assert!(text.ends_with(NOTICE_FOOTER), "{text}");
        assert!(notices.list(None).is_empty(), "a delivered watch is done");
    }

    #[tokio::test]
    async fn notices_that_land_together_share_one_background_event() {
        let fake = Fake::new();
        let notices = started(&fake, None);
        for (chat, message) in [("child-a", "ma"), ("child-b", "mb")] {
            fake.message(chat, message, "user", "go", false);
            fake.status(chat, "working");
            notices.register(Watch::new("parent", chat, message));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        for chat in ["child-a", "child-b"] {
            fake.message(
                chat,
                &format!("{chat}-reply"),
                "assistant",
                &format!("{chat} done"),
                false,
            );
            fake.status(chat, "idle");
        }
        eventually(|| !fake.queued("parent").is_empty()).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let queued = fake.queued("parent");
        assert_eq!(queued.len(), 1, "{queued:?}");
        assert!(queued[0].contains("child-a done") && queued[0].contains("child-b done"));
        assert_eq!(queued[0].matches(NOTICE_FOOTER).count(), 1);

        // Notices go out as background events, never queue rows.
        assert!(
            fake.writes
                .lock()
                .unwrap()
                .iter()
                .all(|(method, _)| method == methods::DELIVER_EVENT)
        );

        // A later notice is its own event.
        fake.drain("parent");
        fake.message("child-b", "mb2", "user", "again", false);
        fake.status("child-b", "working");
        notices.register(Watch::new("parent", "child-b", "mb2"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        fake.message("child-b", "rb2", "assistant", "b again", false);
        fake.status("child-b", "idle");
        eventually(|| !fake.queued("parent").is_empty()).await;
        assert!(fake.queued("parent")[0].contains("b again"));
        assert!(!fake.queued("parent")[0].contains("child-a done"));
    }

    #[tokio::test]
    async fn a_message_held_behind_another_turn_reports_its_own_turn() {
        let fake = Fake::new();
        let notices = started(&fake, None);
        // The chat is busy with someone else's turn; ours waits in its queue.
        fake.message("child-a", "other", "user", "earlier work", false);
        fake.status("child-a", "working");
        notices.register(Watch::new("parent", "child-a", "mine"));
        fake.message(
            "child-a",
            "other-reply",
            "assistant",
            "earlier work done",
            false,
        );
        fake.status("child-a", "idle");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(fake.queued("parent").is_empty(), "that turn was not ours");

        fake.message("child-a", "mine", "user", "my request", false);
        fake.status("child-a", "working");
        fake.message(
            "child-a",
            "mine-reply",
            "assistant",
            "my request done",
            false,
        );
        fake.status("child-a", "idle");
        eventually(|| !fake.queued("parent").is_empty()).await;
        let text = &fake.queued("parent")[0];
        assert!(
            text.contains("my request done") && !text.contains("earlier work done"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_question_is_reported_once_and_the_watch_carries_on() {
        let fake = Fake::new();
        let notices = started(&fake, None);
        fake.message("child-b", "m1", "user", "go", false);
        fake.status("child-b", "working");
        notices.register(Watch::new("parent", "child-b", "m1"));
        fake.state.send_modify(|state| {
            state.transcripts.get_mut("child-b").unwrap().push(json!({
                "id": "ask", "role": "assistant", "createdAt": 2, "deviceId": "dev", "status": "complete",
                "parts": [{ "kind": "input", "id": "i", "requestId": "req-1", "resolved": false,
                    "questions": [{ "id": "q1", "header": "Scope", "question": "Touch the API?",
                        "options": ["Yes", "No"] }] }],
            }));
        });
        fake.status("child-b", "awaitingInput");
        eventually(|| !fake.queued("parent").is_empty()).await;
        let asked = fake.drain("parent").remove(0);
        assert!(
            asked.contains("(child-b) is waiting for an answer"),
            "{asked}"
        );
        assert!(
            asked.contains("Touch the API? (question_id q1): Yes | No"),
            "{asked}"
        );
        assert!(asked.contains("request_id req-1"), "{asked}");
        assert_eq!(
            notices.list(None)[0].reported_input.as_deref(),
            Some("req-1")
        );

        fake.status("child-b", "working");
        fake.message("child-b", "done", "assistant", "Left the API alone.", false);
        fake.status("child-b", "idle");
        eventually(|| !fake.queued("parent").is_empty()).await;
        assert!(fake.queued("parent")[0].contains("Left the API alone."));
        assert!(notices.list(None).is_empty());
    }

    #[tokio::test]
    async fn watches_persist_replace_and_cancel() {
        let dir = std::env::temp_dir().join(format!("turn-notices-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("turn-notices.json");
        {
            let notices = TurnNotices::new(Some(path.clone()));
            notices.register(Watch::new("parent", "child-a", "m1"));
            // A newer send to the same chat replaces the older watch.
            notices.register(Watch::new("parent", "child-a", "m2"));
            notices.register(Watch::new("parent", "child-b", "m3"));
            assert_eq!(notices.list(None).len(), 2);
        }
        let reloaded = TurnNotices::new(Some(path.clone()));
        let watches = reloaded.list(Some("parent"));
        assert_eq!(watches.len(), 2);
        assert!(
            watches
                .iter()
                .any(|w| w.chat_id == "child-a" && w.message_id == "m2")
        );
        assert!(reloaded.cancel("parent", "child-a"));
        assert!(!reloaded.cancel("parent", "child-a"));
        assert_eq!(TurnNotices::new(Some(path)).list(None).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_turn_that_ended_while_the_engine_was_down_still_reports() {
        let fake = Fake::new();
        fake.message("child-a", "m1", "user", "go", false);
        fake.message("child-a", "r1", "assistant", "finished overnight", false);
        fake.status("child-a", "idle");
        let notices = TurnNotices::with_batch_window(None, Duration::from_millis(60));
        notices.register(Watch::new("parent", "child-a", "m1"));
        // Resumed on start.
        notices.start(Arc::new(Zeron::with_client(
            memory_client(fake.clone()),
            crate::Origin::default(),
        )));
        eventually(|| !fake.queued("parent").is_empty()).await;
        assert!(fake.queued("parent")[0].contains("finished overnight"));
    }

    #[tokio::test]
    async fn a_turn_parked_on_background_work_reports_after_its_wake_turn() {
        let fake = Fake::new();
        let notices = started(&fake, None);
        fake.message("child-a", "m1", "user", "run the suite", false);
        fake.status("child-a", "working");
        notices.register(Watch::new("parent", "child-a", "m1"));
        // Claude starts the suite in the background and ends its turn to be
        // woken when it finishes.
        fake.message(
            "child-a",
            "r1",
            "assistant",
            "Started the suite; waiting.",
            false,
        );
        fake.status_with_background("child-a", "idle", 1);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            fake.queued("parent").is_empty(),
            "parked on background work"
        );

        // The suite finishes; the drained count lands just before the wake turn.
        fake.status_with_background("child-a", "idle", 0);
        tokio::time::sleep(Duration::from_millis(300)).await;
        fake.status("child-a", "working");
        tokio::time::sleep(Duration::from_millis(100)).await;
        fake.message(
            "child-a",
            "r2",
            "assistant",
            "Suite passed: 412 tests.",
            false,
        );
        fake.status("child-a", "idle");
        eventually(|| !fake.queued("parent").is_empty()).await;
        let text = &fake.queued("parent")[0];
        assert!(text.contains("Suite passed: 412 tests."), "{text}");
        assert!(!text.contains("Started the suite; waiting."), "{text}");
    }
}
