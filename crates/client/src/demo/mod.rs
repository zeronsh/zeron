//! Demo mode: a fully offline, deterministic workspace with an in-process
//! simulated host.
//!
//! Nothing here short-circuits the client: the dataset lives in a real
//! [`RegistryDoc`] (local writes settle through [`DemoServer`], a stand-in for
//! the registry room) and real [`SessionDoc`]s. The viewer side writes
//! commands and queue rows exactly as in live mode; [`DemoHost`] plays the
//! engine — adopting commands (writing the user entry under the client-minted
//! id), streaming replies through `SegmentWriter`, flipping session status
//! rows, draining the queue, and answering relay RPCs.

mod fixtures;
mod png;
mod transcripts;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, QueueDeliveryGate, RegistryDoc, RegistryRow, RowOp,
    SegmentWriter, SessionCommandPayload, SessionCommandStatus, SessionDoc, apply_op,
};
use zeron_proto::{FolderEntry, FolderListing, RepoRef, Session, SessionStatus};

use crate::catalog::{self, HarnessInfo, ModelInfo};
use crate::client::ClientInner;
use crate::config::{DemoOptions, StreamSpeed};
use crate::error::{ClientError, Result};
use crate::rpc::ProgressFn;
use crate::session::SessionCore;
use crate::workspace::WorkspaceStore;
use crate::{lock, now_ms};

use transcripts::Step;

/// Pretend network latency before the host adopts a command.
const ADOPT_DELAY: Duration = Duration::from_millis(180);
/// Engines heartbeat live session rows well inside the 45s staleness gate.
const HEARTBEAT: Duration = Duration::from_secs(10);
const LEASE_MS: i64 = 60_000;

/// In-process registry room: merges pushed op batches into server rows and
/// broadcasts them back (the mock server's merge, minus the socket).
#[derive(Default)]
pub(crate) struct DemoServer {
    rows: HashMap<(String, String), RegistryRow>,
    seq: u64,
}

impl DemoServer {
    fn settle(&mut self, doc: &mut RegistryDoc) {
        loop {
            let batches = doc.take_pushable();
            if batches.is_empty() {
                return;
            }
            for batch in batches {
                let mut touched = Vec::new();
                for op in &batch.ops {
                    self.apply(op, &mut touched);
                }
                self.seq = self.seq.max(doc.cursor()) + 1;
                let _ = doc.apply_rows(self.seq, touched);
                doc.ack_batch(&batch.batch, self.seq);
            }
        }
    }

    fn apply(&mut self, op: &RowOp, touched: &mut Vec<RegistryRow>) {
        let key = (op.kind.clone(), op.id.clone());
        let (next, changed) = apply_op(self.rows.get(&key), op);
        if let Some(mut row) = next
            && changed
        {
            row.seq = self.seq + 1;
            self.rows.insert(key, row.clone());
            touched.push(row);
        }
    }
}

/// A tiny deterministic PRNG (xorshift64*): demo cadence is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

struct Lease {
    lease_id: String,
    expires_at_ms: i64,
}

pub(crate) struct DemoHost {
    options: DemoOptions,
    client: Weak<ClientInner>,
    server: Mutex<DemoServer>,
    refs: Mutex<HashMap<String, Vec<RepoRef>>>,
    /// Running turn per chat: (turn id, cancel token).
    turns: Mutex<HashMap<String, (u64, CancellationToken)>>,
    turn_seq: AtomicU64,
    /// Serializes turns per chat: a replaced turn finishes its segment first.
    turn_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    processed: Mutex<HashSet<String>>,
    leases: Mutex<HashMap<String, Lease>>,
    uploads: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    rng: Mutex<Rng>,
    cancel: Mutex<Option<CancellationToken>>,
}

fn doc_err(err: zeron_doc::DocError) -> ClientError {
    ClientError::Internal(err.to_string())
}

impl DemoHost {
    pub(crate) fn new(client: &Arc<ClientInner>, options: DemoOptions) -> Arc<Self> {
        Arc::new(Self {
            options,
            client: Arc::downgrade(client),
            server: Mutex::new(DemoServer::default()),
            refs: Mutex::new(HashMap::new()),
            turns: Mutex::new(HashMap::new()),
            turn_seq: AtomicU64::new(1),
            turn_gates: Mutex::new(HashMap::new()),
            processed: Mutex::new(HashSet::new()),
            leases: Mutex::new(HashMap::new()),
            uploads: Mutex::new(HashMap::new()),
            rng: Mutex::new(Rng(0x9e37_79b9_7f4a_7c15)),
            cancel: Mutex::new(None),
        })
    }

    fn client(&self) -> Result<Arc<ClientInner>> {
        self.client.upgrade().ok_or(ClientError::Closed)
    }

    pub(crate) fn seed(&self, client: &Arc<ClientInner>) -> Result<()> {
        let seeded = client
            .workspace
            .mutate(|doc| {
                fixtures::seed(
                    doc,
                    self.options.fixture,
                    &client.config.device_id,
                    &client.config.device_name,
                )
            })
            .map_err(doc_err)?;
        for status in seeded.change_requests {
            client.workspace.put_change_request(status);
        }
        self.settle_registry(&client.workspace);
        self.beat_presence(client);
        Ok(())
    }

    pub(crate) fn settle_registry(&self, workspace: &WorkspaceStore) {
        let mut server = lock(&self.server);
        workspace.mutate(|doc| server.settle(doc));
    }

    fn beat_presence(&self, client: &ClientInner) {
        let now = now_ms();
        for device in fixtures::ONLINE {
            client.workspace.set_presence(device, now);
        }
    }

    /// Heartbeats: device presence + live session rows (so demo Working
    /// never goes stale behind the 45s gate).
    pub(crate) fn start(self: &Arc<Self>, client: &Arc<ClientInner>, cancel: CancellationToken) {
        *lock(&self.cancel) = Some(cancel.clone());
        let host = Arc::downgrade(self);
        let weak = Arc::downgrade(client);
        crate::runtime::shared().spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(HEARTBEAT) => {}
                }
                let (Some(host), Some(client)) = (host.upgrade(), weak.upgrade()) else {
                    return;
                };
                host.beat_presence(&client);
                let (state, _) = client.workspace.state();
                let live: Vec<Session> = state
                    .sessions
                    .iter()
                    .filter(|s| matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput))
                    .cloned()
                    .collect();
                if !live.is_empty() {
                    let _ = client.registry_write(|doc| {
                        for mut session in live {
                            session.updated_at = Utc::now();
                            doc.upsert_session(&session)?;
                        }
                        Ok(())
                    });
                } else {
                    client.recompute_workspace();
                }
            }
        });
    }

    pub(crate) fn stop(&self) {
        for (_, (_, token)) in lock(&self.turns).drain() {
            token.cancel();
        }
    }

    fn cancel_token(&self) -> CancellationToken {
        lock(&self.cancel).clone().unwrap_or_default()
    }

    fn host_of(&self, client: &ClientInner, chat_id: &str) -> String {
        client
            .workspace
            .chat(chat_id)
            .map(|c| c.device_id)
            .unwrap_or_else(|| fixtures::MAC.to_owned())
    }

    fn host_online(&self, client: &ClientInner, device_id: &str) -> bool {
        crate::workspace::device_online(
            device_id,
            &client.workspace.presence(),
            &client.config.device_id,
            now_ms(),
        )
    }

    /// The fixture doc for a chat, written the way a host would.
    pub(crate) fn session_doc(&self, chat_id: &str) -> Result<SessionDoc> {
        let client = self.client()?;
        let doc = SessionDoc::init(chat_id).map_err(doc_err)?;
        let host = self.host_of(&client, chat_id);
        let chat = client.workspace.chat(chat_id);
        let last = chat
            .as_ref()
            .and_then(|c| c.last_message_at)
            .map_or_else(now_ms, |t| t.timestamp_millis());
        let entries = match (chat_id, self.options.transcript_scale.turns()) {
            ("chat-veil", Some(turns)) => {
                transcripts::synthetic(turns, &host, now_ms() - turns as i64 * 60_000)
            }
            _ => transcripts::fixture(chat_id, &host, last),
        };
        for entry in &entries {
            doc.push_message(entry).map_err(doc_err)?;
        }
        Ok(doc)
    }

    /// A viewer opened `chat-veil`: finish its in-flight streaming entry.
    pub(crate) fn session_opened(self: &Arc<Self>, core: &Arc<SessionCore>) {
        let snapshot = core.snapshot();
        let Some(last) = snapshot.entries.get(..snapshot.transcript_len).and_then(|e| e.last()) else {
            return;
        };
        if !last.is_streaming() || lock(&self.turns).contains_key(&core.chat_id) {
            return;
        }
        let steps = vec![Step::Text(transcripts::VEIL_LIVE_REST.to_owned())];
        let written = last.message.parts.clone();
        let index = core.doc().doc().get_list("messages").len().saturating_sub(1);
        self.spawn_turn(core.chat_id.clone(), None, steps, Some((index, written)));
    }

    // ── command plane ──────────────────────────────────────────────────────

    pub(crate) fn on_command(self: &Arc<Self>, chat_id: &str) {
        let host = self.clone();
        let chat_id = chat_id.to_owned();
        let cancel = self.cancel_token();
        crate::runtime::shared().spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(ADOPT_DELAY) => {}
            }
            host.drain(&chat_id);
        });
    }

    /// The network came back: adopt everything that queued meanwhile.
    pub(crate) fn network_recovered(self: &Arc<Self>) {
        let Ok(client) = self.client() else { return };
        let (state, _) = client.workspace.state();
        for chat in &state.chats {
            if client.session_core(&chat.id).is_some() {
                self.on_command(&chat.id);
            }
        }
    }

    fn drain(self: &Arc<Self>, chat_id: &str) {
        let Ok(client) = self.client() else { return };
        if !client.network_online() {
            return;
        }
        let host = self.host_of(&client, chat_id);
        if !self.host_online(&client, &host) {
            return;
        }
        let Some(core) = client.session_core(chat_id) else { return };
        let commands = core.doc().read_commands().unwrap_or_default();
        for command in commands {
            if command.status != SessionCommandStatus::Pending
                || !lock(&self.processed).insert(command.id.clone())
            {
                continue;
            }
            let _ = core.write(|doc| {
                doc.set_command_status(&command.id, SessionCommandStatus::Applied, None)
            });
            match command.payload {
                SessionCommandPayload::Run {
                    request,
                    message_id,
                } => {
                    let prompt = self.adopt_attachments(&client, &host, &request.prompt);
                    let steps = self.reply_for(&prompt);
                    self.spawn_turn(
                        chat_id.to_owned(),
                        Some((message_id, prompt, command.issued_by.clone())),
                        steps,
                        None,
                    );
                }
                SessionCommandPayload::Steer { prompt, message_id } => {
                    let prompt = self.adopt_attachments(&client, &host, &prompt);
                    let steps = self.reply_for(&prompt);
                    let id = message_id.unwrap_or_else(crate::new_id);
                    self.spawn_turn(
                        chat_id.to_owned(),
                        Some((id, prompt, command.issued_by.clone())),
                        steps,
                        None,
                    );
                }
                SessionCommandPayload::Interrupt {} => {
                    if let Some((_, token)) = lock(&self.turns).remove(chat_id) {
                        token.cancel();
                    } else {
                        self.set_status(&client, chat_id, SessionStatus::Idle, None);
                    }
                }
                SessionCommandPayload::RespondInput {
                    request_id,
                    answers,
                } => {
                    let _ = core.write(|doc| doc.resolve_input(&request_id));
                    let labels: Vec<String> =
                        answers.into_iter().flat_map(|a| a.labels).collect();
                    self.spawn_turn(chat_id.to_owned(), None, transcripts::answered(&labels), None);
                }
            }
        }
        if !lock(&self.turns).contains_key(chat_id) {
            self.drain_queue(chat_id);
        }
    }

    fn reply_for(&self, prompt: &str) -> Vec<Step> {
        if prompt.contains("?ask") {
            transcripts::asking()
        } else {
            transcripts::reply(prompt, self.options.long_reply)
        }
    }

    /// Resolve `pending://` refs to host paths (the host's dispatch rewrite)
    /// and carry the staged bytes over.
    fn adopt_attachments(&self, client: &ClientInner, host: &str, prompt: &str) -> String {
        let mut out = prompt.to_owned();
        for line in prompt.lines() {
            let Some(reference) = line.trim().strip_prefix("- ") else { continue };
            let Some((upload_id, name)) = crate::attachments::parse_pending_ref(reference) else {
                continue;
            };
            let path = format!("/Users/dev/.zeron/uploads/{upload_id}-{name}");
            if let Some(bytes) = client.attachment_cache.get(host, reference) {
                lock(&self.uploads).insert(path.clone(), bytes.clone());
                client.attachment_cache.put(host, &path, bytes);
            }
            out = out.replace(reference, &path);
        }
        out
    }

    fn drain_queue(self: &Arc<Self>, chat_id: &str) {
        let Ok(client) = self.client() else { return };
        let Some(core) = client.session_core(chat_id) else { return };
        let Ok(Some(item)) = core.write(|doc| doc.take_queue_head()) else {
            return;
        };
        let host = self.host_of(&client, chat_id);
        let text = if item.attachments.is_empty() || item.text.contains(crate::attachments::ATTACHMENT_MARKER) {
            item.text.clone()
        } else {
            crate::attachments::with_attachments(&item.text, &item.attachments)
        };
        let prompt = self.adopt_attachments(&client, &host, &text);
        let steps = self.reply_for(&prompt);
        self.spawn_turn(chat_id.to_owned(), Some((item.id, prompt, item.issued_by)), steps, None);
    }

    fn set_status(&self, client: &Arc<ClientInner>, chat_id: &str, status: SessionStatus, completed: Option<&str>) {
        let host = self.host_of(client, chat_id);
        let now = Utc::now();
        let started = client
            .workspace
            .state()
            .0
            .sessions
            .iter()
            .find(|s| s.chat_id == chat_id)
            .and_then(|s| s.started_at);
        let _ = client.registry_write(|doc| {
            doc.upsert_session(&Session {
                last_completed_turn: completed.map(str::to_owned),
                chat_id: chat_id.to_owned(),
                device_id: host,
                status,
                started_at: match status {
                    SessionStatus::Working => Some(now),
                    SessionStatus::AwaitingInput => started.or(Some(now)),
                    _ => None,
                },
                updated_at: now,
            })
        });
    }

    fn cadence(&self, reasoning: bool) -> Duration {
        let ms = match self.options.stream_speed {
            StreamSpeed::Fast => 4,
            StreamSpeed::Realistic => lock(&self.rng).range(30, 140),
        };
        Duration::from_millis(if reasoning { ms / 3 + 1 } else { ms })
    }

    /// Start (replacing any running) turn on `chat_id`. Turns on one chat run
    /// strictly one after another: a replaced turn aborts and finishes its
    /// segment before the next one writes.
    fn spawn_turn(
        self: &Arc<Self>,
        chat_id: String,
        user: Option<(String, String, String)>,
        steps: Vec<Step>,
        resume: Option<(usize, Vec<MessagePart>)>,
    ) {
        let root = self.cancel_token();
        if root.is_cancelled() {
            return;
        }
        let token = root.child_token();
        let turn_id = self.turn_seq.fetch_add(1, Ordering::Relaxed);
        if let Some((_, previous)) = lock(&self.turns).insert(chat_id.clone(), (turn_id, token.clone())) {
            previous.cancel();
        }
        let gate = lock(&self.turn_gates)
            .entry(chat_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let host = self.clone();
        crate::runtime::shared().spawn(async move {
            let _serial = gate.lock().await;
            if !token.is_cancelled() {
                host.run_turn(&chat_id, turn_id, user, steps, resume, token).await;
            }
            let idle = {
                let mut turns = lock(&host.turns);
                if turns.get(&chat_id).is_some_and(|(id, _)| *id == turn_id) {
                    turns.remove(&chat_id);
                }
                !turns.contains_key(&chat_id)
            };
            if idle && !root.is_cancelled() {
                host.drain_queue(&chat_id);
            }
        });
    }

    fn is_current_turn(&self, chat_id: &str, turn_id: u64) -> bool {
        lock(&self.turns)
            .get(chat_id)
            .is_none_or(|(id, _)| *id == turn_id)
    }

    async fn run_turn(
        self: &Arc<Self>,
        chat_id: &str,
        turn_id: u64,
        user: Option<(String, String, String)>,
        steps: Vec<Step>,
        resume: Option<(usize, Vec<MessagePart>)>,
        token: CancellationToken,
    ) {
        let Ok(client) = self.client() else { return };
        let Some(core) = client.session_core(chat_id) else { return };
        let host = self.host_of(&client, chat_id);
        let now = now_ms();

        if let Some((message_id, prompt, author)) = &user {
            let adopted = core
                .snapshot()
                .entry(message_id)
                .is_some_and(|e| e.echo.is_none());
            if !adopted {
                let entry = transcripts::entry(
                    message_id,
                    MessageRole::User,
                    author,
                    now,
                    vec![transcripts::text("t0", prompt)],
                );
                let _ = core.write(|doc| doc.push_message(&entry));
            }
            let preview = crate::attachments::parse_user_message(prompt).text;
            let untitled = client.workspace.chat(chat_id).is_some_and(|c| c.title.is_none());
            let _ = client.registry_write(|doc| {
                doc.set_chat_last_message(chat_id, &zeron_proto::view::single_line(&preview), Utc::now())?;
                if untitled {
                    let title: String = zeron_proto::view::single_line(&preview).chars().take(48).collect();
                    doc.rename_chat(chat_id, if title.is_empty() { "Image" } else { &title })?;
                }
                Ok(())
            });
        }
        self.set_status(&client, chat_id, SessionStatus::Working, None);

        let (mut index, mut written) = match resume {
            Some(state) => state,
            None => {
                let entry_id = crate::new_id();
                match core.write(|doc| Ok(SegmentWriter::begin(doc, &entry_id, &host, now_ms())?.into_state())) {
                    Ok(state) => state,
                    Err(_) => return,
                }
            }
        };
        let mut parts = written.clone();
        let entry_id = core
            .snapshot()
            .entries
            .get(..core.snapshot().transcript_len)
            .and_then(|e| e.last())
            .map(|e| e.id.clone())
            .unwrap_or_default();

        let sync = |parts: &[MessagePart], written: &mut Vec<MessagePart>, index: &mut usize| {
            let taken = std::mem::take(written);
            let at = *index;
            match core.write(|doc| {
                let mut writer = SegmentWriter::resume(doc, at, taken);
                writer.sync(parts)?;
                Ok(writer.into_state())
            }) {
                Ok((i, w)) => {
                    *index = i;
                    *written = w;
                }
                Err(err) => tracing::warn!(error = %err, "demo stream write failed"),
            }
        };

        let mut awaiting = false;
        let mut aborted = false;
        'steps: for step in steps {
            match step {
                Step::Reasoning(body) | Step::Text(body) if body.is_empty() => {}
                Step::Reasoning(body) => {
                    parts.push(MessagePart::Reasoning {
                        id: format!("r{}", parts.len()),
                        text: String::new(),
                    });
                    for word in words(&body) {
                        if token.is_cancelled() {
                            aborted = true;
                            break 'steps;
                        }
                        if let Some(MessagePart::Reasoning { text, .. }) = parts.last_mut() {
                            text.push_str(word);
                        }
                        sync(&parts, &mut written, &mut index);
                        tokio::time::sleep(self.cadence(true)).await;
                    }
                }
                Step::Text(body) => {
                    let continuing = matches!(parts.last(), Some(MessagePart::Text { .. }));
                    if !continuing {
                        parts.push(MessagePart::Text {
                            id: format!("t{}", parts.len()),
                            text: String::new(),
                        });
                    }
                    for word in words(&body) {
                        if token.is_cancelled() {
                            aborted = true;
                            break 'steps;
                        }
                        if let Some(MessagePart::Text { text, .. }) = parts.last_mut() {
                            text.push_str(word);
                        }
                        sync(&parts, &mut written, &mut index);
                        tokio::time::sleep(self.cadence(false)).await;
                    }
                }
                Step::Tool {
                    call,
                    output,
                    is_error,
                    run_ms,
                } => {
                    let mut part = transcripts::tool(&format!("k{}", parts.len()), call, is_error, output.as_deref());
                    if let MessagePart::Tool { resolved, .. } = &mut part {
                        *resolved = false;
                    }
                    parts.push(part);
                    sync(&parts, &mut written, &mut index);
                    let wait = match self.options.stream_speed {
                        StreamSpeed::Fast => run_ms / 20,
                        StreamSpeed::Realistic => run_ms,
                    };
                    tokio::select! {
                        _ = token.cancelled() => { aborted = true; break 'steps; }
                        _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                    }
                    if let Some(MessagePart::Tool { resolved, .. }) = parts.last_mut() {
                        *resolved = true;
                    }
                    sync(&parts, &mut written, &mut index);
                }
                Step::Question(questions) => {
                    let request_id = crate::new_id();
                    parts.push(MessagePart::Input {
                        id: request_id.clone(),
                        request_id,
                        questions,
                        resolved: false,
                    });
                    sync(&parts, &mut written, &mut index);
                    awaiting = true;
                }
            }
        }

        let status = if aborted {
            MessageStatus::Aborted
        } else {
            MessageStatus::Complete
        };
        let final_parts = parts.clone();
        let _ = core.write(|doc| SegmentWriter::resume(doc, index, written).finish(&final_parts, status));
        let preview = parts
            .iter()
            .rev()
            .find_map(|p| match p {
                MessagePart::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            })
            .map(|t| {
                zeron_proto::view::single_line(t.lines().find(|l| !l.trim().is_empty()).unwrap_or(""))
            })
            .unwrap_or_default();
        let viewing = core.snapshot().revision > 0 && self.is_viewing(&core);
        let _ = client.registry_write(|doc| {
            if !preview.is_empty() {
                doc.set_chat_last_message(chat_id, &preview, Utc::now())?;
            }
            if viewing {
                doc.set_chat_seen(chat_id, Utc::now() + chrono::Duration::milliseconds(1))?;
            }
            Ok(())
        });
        let status = if awaiting {
            SessionStatus::AwaitingInput
        } else {
            SessionStatus::Idle
        };
        self.set_status(&client, chat_id, status, (!aborted && !awaiting).then_some(entry_id.as_str()));
    }

    fn is_viewing(&self, core: &SessionCore) -> bool {
        core.view_attached()
    }

    // ── relay RPCs ─────────────────────────────────────────────────────────

    pub(crate) async fn host_rpc(
        self: &Arc<Self>,
        device_id: &str,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        use zeron_rpc::methods as m;
        let client = self.client()?;
        if !client.network_online() || !self.host_online(&client, device_id) {
            return Err(ClientError::HostUnavailable(device_id.to_owned()));
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
        let chat_id = params["chatId"].as_str().unwrap_or_default().to_owned();
        let id = params["id"].as_str().unwrap_or_default().to_owned();
        let core = client
            .session_core(&chat_id)
            .ok_or_else(|| ClientError::NotFound(chat_id.clone()))?;
        let row = core
            .doc()
            .read_queue()
            .unwrap_or_default()
            .into_iter()
            .find(|q| q.id == id);
        let now = now_ms();
        let reply = match method {
            m::BEGIN_QUEUED_MESSAGE_EDIT => match row {
                None => serde_json::json!({ "outcome": "missing" }),
                Some(row) if row.delivery_gate.is_some() => serde_json::json!({ "outcome": "locked" }),
                Some(row) => {
                    let lease_id = crate::new_id();
                    let hash = text_hash(&row.text);
                    let gate = QueueDeliveryGate::Editing {
                        lease_id: lease_id.clone(),
                        owner_device_id: params["editorDeviceId"].as_str().unwrap_or_default().into(),
                        owner_instance_id: params["editorInstanceId"].as_str().unwrap_or_default().into(),
                        acquired_at_ms: now,
                        expires_at_ms: now + LEASE_MS,
                        base_text_hash: hash.clone(),
                    };
                    core.write(|doc| doc.set_queued_delivery_gate(&id, Some(&gate)))?;
                    lock(&self.leases).insert(
                        id.clone(),
                        Lease {
                            lease_id: lease_id.clone(),
                            expires_at_ms: now + LEASE_MS,
                        },
                    );
                    serde_json::json!({
                        "outcome": "acquired",
                        "leaseId": lease_id,
                        "text": row.text,
                        "baseTextHash": hash,
                        "expiresAtMs": now + LEASE_MS,
                    })
                }
            },
            m::RENEW_QUEUED_MESSAGE_EDIT => {
                let mut leases = lock(&self.leases);
                match leases.get_mut(&id) {
                    Some(lease) if Some(lease.lease_id.as_str()) == params["leaseId"].as_str() => {
                        lease.expires_at_ms = now + LEASE_MS;
                        serde_json::json!({ "outcome": "renewed" })
                    }
                    _ => serde_json::json!({ "outcome": "lost" }),
                }
            }
            m::FINISH_QUEUED_MESSAGE_EDIT => {
                let held = lock(&self.leases)
                    .get(&id)
                    .is_some_and(|l| Some(l.lease_id.as_str()) == params["leaseId"].as_str());
                match (row, held) {
                    (None, _) => serde_json::json!({ "outcome": "missing" }),
                    (Some(_), false) => serde_json::json!({ "outcome": "lost" }),
                    (Some(row), true) if params["expectedTextHash"].as_str() != Some(text_hash(&row.text).as_str()) => {
                        serde_json::json!({ "outcome": "conflict", "currentText": row.text })
                    }
                    (Some(_), true) => {
                        lock(&self.leases).remove(&id);
                        let outcome = match params["action"].as_str() {
                            Some("commit") => {
                                let text = params["text"].as_str().unwrap_or_default().to_owned();
                                core.write(|doc| doc.finish_queued_edit(&id, Some(&text), now))?;
                                "committed"
                            }
                            Some("discard") => {
                                core.write(|doc| doc.remove_queued(&id))?;
                                "discarded"
                            }
                            Some("cancel") => {
                                core.write(|doc| doc.finish_queued_edit(&id, None, now))?;
                                "cancelled"
                            }
                            _ => {
                                core.write(|doc| doc.finish_queued_edit(&id, None, now))?;
                                "released"
                            }
                        };
                        if !lock(&self.turns).contains_key(&chat_id) {
                            self.drain_queue(&chat_id);
                        }
                        serde_json::json!({ "outcome": outcome })
                    }
                }
            }
            m::SEND_QUEUED_MESSAGE_NOW => {
                let taken = core.write(|doc| doc.take_queued(&id))?;
                match taken {
                    Some(item) => {
                        let host = self.host_of(&client, &chat_id);
                        let text = if item.attachments.is_empty() {
                            item.text.clone()
                        } else {
                            crate::attachments::with_attachments(&item.text, &item.attachments)
                        };
                        let prompt = self.adopt_attachments(&client, &host, &text);
                        let steps = self.reply_for(&prompt);
                        self.spawn_turn(chat_id.clone(), Some((item.id, prompt, item.issued_by)), steps, None);
                        serde_json::json!({ "sent": true })
                    }
                    None => serde_json::json!({ "sent": false }),
                }
            }
            m::REMOVE_QUEUED_MESSAGE => {
                let removed = core.write(|doc| doc.remove_queued(&id))?;
                serde_json::json!({ "removed": removed })
            }
            other => return Err(ClientError::Unsupported(other.to_owned())),
        };
        Ok(reply)
    }

    pub(crate) async fn list_harnesses(&self, _device_id: &str) -> Vec<HarnessInfo> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut list = catalog::fallback_harnesses();
        for harness in &mut list {
            harness.supports_steering = Some(true);
            harness.steering_mode = Some("step-boundary".into());
            harness.enabled = Some(true);
        }
        list.push(HarnessInfo {
            id: "opencode".into(),
            label: "OpenCode".into(),
            supports_steering: Some(false),
            steering_mode: Some("turn-boundary".into()),
            reasoning_levels: Vec::new(),
            installed: true,
            enabled: Some(true),
        });
        list
    }

    pub(crate) async fn list_models(&self, harness: &str) -> Vec<ModelInfo> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        catalog::fallback_models(harness)
    }

    fn seeded_refs(path: &str) -> Vec<RepoRef> {
        let r = |name: &str, current: bool, worktree: Option<&str>| RepoRef {
            name: name.into(),
            current,
            worktree_path: worktree.map(str::to_owned),
        };
        if path.contains("zeron") {
            vec![
                r("main", true, None),
                r("veil-fade", false, Some("/Users/dev/.zeron/worktrees/zeron-veil-fade")),
                r("feature/diff-pane", false, None),
                r("fix/tool-colors", false, None),
            ]
        } else {
            vec![r("main", true, None), r("staging", false, None)]
        }
    }

    pub(crate) async fn list_refs(&self, path: &str) -> Result<Vec<RepoRef>> {
        tokio::time::sleep(Duration::from_millis(120)).await;
        Ok(lock(&self.refs)
            .entry(path.to_owned())
            .or_insert_with(|| Self::seeded_refs(path))
            .clone())
    }

    pub(crate) async fn switch_ref(&self, path: &str, name: &str) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut refs = lock(&self.refs);
        let list = refs
            .entry(path.to_owned())
            .or_insert_with(|| Self::seeded_refs(path));
        if !list.iter().any(|r| r.name == name) {
            return Err(ClientError::NotFound(name.to_owned()));
        }
        for r in list.iter_mut() {
            r.current = r.name == name;
        }
        Ok(())
    }

    pub(crate) async fn create_worktree(&self, path: &str, base: &str) -> Result<String> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let folder = path.rsplit('/').next().unwrap_or("repo");
        let worktree = format!(
            "/Users/dev/.zeron/worktrees/{folder}-{}",
            base.replace('/', "-")
        );
        let mut refs = lock(&self.refs);
        let list = refs
            .entry(path.to_owned())
            .or_insert_with(|| Self::seeded_refs(path));
        if let Some(r) = list.iter_mut().find(|r| r.name == base && r.worktree_path.is_none()) {
            r.worktree_path = Some(worktree.clone());
        }
        Ok(worktree)
    }

    pub(crate) async fn list_folders(&self, device_id: &str, path: Option<String>) -> Result<FolderListing> {
        tokio::time::sleep(Duration::from_millis(120)).await;
        let home = if device_id == fixtures::VPS { "/srv" } else { "/Users/dev" };
        let path = path.unwrap_or_else(|| home.to_owned());
        let names: &[&str] = match path.as_str() {
            "/Users/dev" => &["Documents", "Downloads", "Projects", "scratch", "zeron", "zeron-ios"],
            "/Users/dev/Documents" => &["notes", "specs"],
            "/Users/dev/Projects" => &["blog", "dotfiles", "playground", "zeron"],
            "/Users/dev/Projects/zeron" => &["apps", "crates", "docs", "edge"],
            "/Users/dev/Projects/blog" => &["content", "public"],
            "/srv" => &["backups", "deploys"],
            "/srv/deploys" => &["edge", "landing"],
            _ => &[],
        };
        const REPOS: &[&str] = &["zeron", "zeron-ios", "dotfiles", "blog", "playground", "edge", "landing"];
        Ok(FolderListing {
            path,
            entries: names
                .iter()
                .map(|name| FolderEntry {
                    name: (*name).into(),
                    is_dir: true,
                    is_repo: REPOS.contains(name),
                })
                .collect(),
            truncated: false,
        })
    }

    pub(crate) async fn upload(
        &self,
        client: &Arc<ClientInner>,
        device_id: &str,
        name: &str,
        data: Vec<u8>,
        progress: Option<ProgressFn>,
    ) -> Result<String> {
        for step in 1..=4 {
            tokio::time::sleep(Duration::from_millis(60)).await;
            if let Some(progress) = &progress {
                progress(step as f64 / 4.0);
            }
        }
        let path = format!("/Users/dev/.zeron/uploads/{}-{name}", &crate::new_id()[..8]);
        let bytes = Arc::new(data);
        lock(&self.uploads).insert(path.clone(), bytes.clone());
        client.attachment_cache.put(device_id, &path, bytes);
        Ok(path)
    }

    pub(crate) fn read_attachment(&self, path: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(bytes) = lock(&self.uploads).get(path) {
            return Ok(bytes.clone());
        }
        transcripts::DEMO_IMAGES
            .iter()
            .find(|(p, ..)| *p == path)
            .map(|(_, w, h, seed)| Arc::new(png::gradient(*w, *h, *seed)))
            .ok_or_else(|| ClientError::NotFound(path.to_owned()))
    }
}

/// Word-granular chunks, whitespace attached to the preceding word.
fn words(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\n' {
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\n') {
                i += 1;
            }
            out.push(&body[start..i]);
            start = i;
        } else {
            i += 1;
        }
    }
    if start < body.len() {
        out.push(&body[start..]);
    }
    out
}

fn text_hash(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}
