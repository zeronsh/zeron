//! Durable native fork transactions. Provider creation is never retried after an
//! ambiguous result; a durable ProviderCreated record can always finish publication.
use crate::{DocHost, HarnessRegistry, SessionsEngine, WorkspaceHost};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use zeron_doc::{
    HistoryStrategy, MessagePart, MessageRole, MessageStatus, NativeForkLineage,
    SessionMessageEntry, join_continuation_entries,
};
use zeron_proto::*;

#[derive(Clone)]
pub struct NativeForks(Arc<Inner>);
struct Inner {
    dir: PathBuf,
    docs: DocHost,
    workspace: WorkspaceHost,
    registry: Arc<HarnessRegistry>,
    sessions: SessionsEngine,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    support: tokio::sync::Mutex<HashMap<String, NativeForkAvailability>>,
    cancel: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Phase {
    Prepared,
    ProviderCreated,
    Published,
    Failed,
    Indeterminate,
}
#[derive(Clone, Serialize, Deserialize)]
struct Operation {
    request: ForkMessageSideChatRequest,
    phase: Phase,
    chat: Chat,
    prefix: Vec<SessionMessageEntry>,
    point: NativeForkPoint,
    child: Option<NativeForkResult>,
    error: Option<String>,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

impl NativeForks {
    pub fn new(
        dir: PathBuf,
        docs: DocHost,
        workspace: WorkspaceHost,
        registry: Arc<HarnessRegistry>,
        sessions: SessionsEngine,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(err)?;
        let this = Self(Arc::new(Inner {
            dir,
            docs,
            workspace,
            registry,
            sessions,
            locks: Mutex::new(HashMap::new()),
            support: Default::default(),
            cancel: Default::default(),
            tasks: Default::default(),
        }));
        for path in std::fs::read_dir(&this.0.dir).map_err(err)? {
            let path = path.map_err(err)?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let mut op: Operation =
                serde_json::from_slice(&std::fs::read(path).map_err(err)?).map_err(err)?;
            match op.phase {
                Phase::Prepared => {
                    op.phase = Phase::Indeterminate;
                    op.error = Some("Engine stopped before the provider result was recorded; creation will not be retried".into());
                    this.save(&op)?;
                }
                Phase::ProviderCreated => {
                    if let Err(error) = this.publish(&mut op) {
                        tracing::warn!(%error, "native fork publication recovery deferred");
                    }
                }
                _ => {}
            }
        }
        Ok(this)
    }
    fn lock(&self, key: String) -> Arc<tokio::sync::Mutex<()>> {
        self.0
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default()
            .clone()
    }
    fn path(&self, request: &str) -> PathBuf {
        use sha2::Digest;
        self.0.dir.join(format!(
            "{:x}.json",
            sha2::Sha256::digest(request.as_bytes())
        ))
    }
    fn save(&self, op: &Operation) -> Result<(), String> {
        use std::io::Write;
        let target = self.path(&op.request.request_id);
        let temporary = target.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(err)?;
        file.write_all(&serde_json::to_vec(op).map_err(err)?)
            .map_err(err)?;
        file.sync_all().map_err(err)?;
        std::fs::rename(temporary, target).map_err(err)?;
        #[cfg(unix)]
        std::fs::File::open(&self.0.dir)
            .and_then(|f| f.sync_all())
            .map_err(err)?;
        Ok(())
    }
    fn source(&self, id: &str, device: &str) -> Result<(Chat, Vec<SessionMessageEntry>), String> {
        if device != self.0.docs.device_id() {
            return Err("Fork must run on the source host".into());
        }
        let source = self
            .0
            .workspace
            .chat(id)
            .map_err(err)?
            .ok_or("Source chat no longer exists")?;
        if source.device_id != device {
            return Err("Source belongs to another device".into());
        }
        let entries = self
            .0
            .docs
            .open(id)
            .map_err(err)?
            .doc()
            .read_entries()
            .map_err(err)?;
        Ok((source, entries))
    }
    fn point(&self, source: &Chat, entry: &SessionMessageEntry) -> Result<NativeForkPoint, String> {
        if entry.role != MessageRole::Assistant || entry.status != Some(MessageStatus::Complete) {
            return Err("This reply is not complete".into());
        }
        let point = entry
            .native_fork_point
            .clone()
            .ok_or("Native fork point unavailable for this message")?;
        point.validate()?;
        if !native_fork_provider(point.harness) && point.harness != HarnessId::Mock {
            return Err("Provider does not support native forks".into());
        }
        if point.source_device_id != source.device_id
            || source
                .config
                .as_ref()
                .is_some_and(|c| c.harness != point.harness)
            || source.cwd.as_deref().is_some_and(|cwd| {
                crate::repos::expand_home(cwd).ok().as_deref() != Some(point.cwd.as_str())
            })
        {
            return Err("Native fork provenance does not match this chat".into());
        }
        Ok(point)
    }
    async fn support(&self, point: &NativeForkPoint) -> NativeForkAvailability {
        let registry = &self.0.registry;
        if registry.update_pending(point.harness) {
            return NativeForkAvailability::unavailable("Provider update in progress");
        }
        let harness = match registry.resolve(point.harness) {
            Ok(h) => h,
            Err(e) => return NativeForkAvailability::unavailable(e.to_string()),
        };
        let identity = harness
            .model_context()
            .ok()
            .flatten()
            .map(|m| m.hash)
            .unwrap_or_default();
        let key = format!("{:?}:{identity}:{}", point.harness, point.cwd);
        let mut cache = self.0.support.lock().await;
        if let Some(result) = cache.get(&key) {
            return result.clone();
        }
        let _lease = registry.execution_lease(point.harness).await;
        let result = harness.native_fork_support(Path::new(&point.cwd)).await;
        if result.available {
            cache.insert(key, result.clone());
        }
        result
    }
    pub async fn availability(
        &self,
        request: NativeForkAvailabilityRequest,
    ) -> Result<HashMap<String, NativeForkAvailability>, String> {
        if request.message_ids.len() > 512 {
            return Err("Too many native fork messages".into());
        }
        let (source, entries) = self.source(&request.source_chat_id, &request.target_device_id)?;
        let entries = join_continuation_entries(entries);
        let mut result = HashMap::new();
        for id in request.message_ids {
            let point = entries
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| "Message no longer exists".to_string())
                .and_then(|e| self.point(&source, e));
            let available = match point {
                Ok(p) => self.support(&p).await,
                Err(reason) => NativeForkAvailability::unavailable(reason),
            };
            result.insert(id, available);
        }
        Ok(result)
    }
    pub async fn create(&self, request: ForkMessageSideChatRequest) -> Result<Chat, String> {
        if self.0.tasks.is_closed() {
            return Err("Engine is shutting down".into());
        }
        let this = self.clone();
        self.0
            .tasks
            .spawn(async move { this.execute(request).await })
            .await
            .map_err(err)?
    }
    async fn execute(&self, request: ForkMessageSideChatRequest) -> Result<Chat, String> {
        if !valid_id(&request.request_id) || !valid_id(&request.chat_id) {
            return Err("Invalid fork operation identity".into());
        }
        let _operation = self
            .lock(format!("request:{}", request.request_id))
            .lock_owned()
            .await;
        let _destination = self
            .lock(format!("destination:{}", request.chat_id))
            .lock_owned()
            .await;
        let path = self.path(&request.request_id);
        if path.exists() {
            let mut op: Operation =
                serde_json::from_slice(&std::fs::read(path).map_err(err)?).map_err(err)?;
            if op.request != request {
                return Err("Fork request ID was already used with another payload".into());
            }
            return match op.phase {
                Phase::Published => Ok(op.chat),
                Phase::ProviderCreated => {
                    self.publish(&mut op)?;
                    Ok(op.chat)
                }
                Phase::Failed => {
                    // Only a definite pre-creation rejection is safe to retry.
                    // Keep the original frozen prefix and destination identity.
                    op.error = None;
                    op.phase = Phase::Prepared;
                    self.save(&op)?;
                    self.create_provider(op).await
                }
                _ => Err(op.error.unwrap_or_else(|| {
                    "Native fork outcome is indeterminate; creation will not be repeated".into()
                })),
            };
        }
        // Reserve the destination across requests, including ambiguous operations.
        for path in std::fs::read_dir(&self.0.dir).map_err(err)? {
            let path = path.map_err(err)?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let op: Operation =
                    serde_json::from_slice(&std::fs::read(path).map_err(err)?).map_err(err)?;
                if op.request.chat_id == request.chat_id {
                    return Err("Destination is reserved by another fork operation".into());
                }
            }
        }
        if self
            .0
            .workspace
            .chat(&request.chat_id)
            .map_err(err)?
            .is_some()
        {
            return Err("Chat id already exists".into());
        }
        let (source, entries) = self.source(&request.source_chat_id, &request.target_device_id)?;
        let joined = join_continuation_entries(entries.clone());
        let response = joined
            .iter()
            .find(|e| e.id == request.source_message_id)
            .ok_or("Source response no longer exists")?;
        let point = self.point(&source, response)?;
        let support = self.support(&point).await;
        if !support.available {
            return Err(support.reason.unwrap_or_default());
        }
        let parent = source
            .parent_chat_id
            .clone()
            .unwrap_or_else(|| source.id.clone());
        if request.destination == NativeForkDestination::MainConversation
            && request.parent_chat_id.is_some()
        {
            return Err("Main conversation forks cannot have a visual parent".into());
        }
        if request
            .parent_chat_id
            .as_ref()
            .is_some_and(|id| id != &parent)
        {
            return Err("Invalid visual parent for this fork".into());
        }
        let parent_row = self
            .0
            .workspace
            .chat(&parent)
            .map_err(err)?
            .ok_or("Fork parent no longer exists")?;
        if parent_row.device_id != source.device_id || parent_row.space_id != source.space_id {
            return Err("Fork parent belongs to another space or device".into());
        }
        let boundary = entries
            .iter()
            .rposition(|e| {
                e.id == request.source_message_id
                    || e.continuation_of.as_deref() == Some(&request.source_message_id)
            })
            .ok_or("Source boundary disappeared")?;
        let mut chat = source.clone();
        chat.id = request.chat_id.clone();
        chat.parent_chat_id = match request.destination {
            NativeForkDestination::SideChat => Some(parent),
            NativeForkDestination::MainConversation => None,
        };
        chat.title = None;
        chat.archived = false;
        chat.created_at = chrono::Utc::now();
        chat.last_message_at = None;
        chat.last_message_preview = None;
        chat.last_seen_at = None;
        chat.room_gen = Some(2);
        chat.harness_session_id = None;
        chat.harness_session_cwd = None;
        let op = Operation {
            request,
            phase: Phase::Prepared,
            chat,
            prefix: entries[..=boundary].to_vec(),
            point,
            child: None,
            error: None,
        };
        self.save(&op)?;
        self.create_provider(op).await
    }
    async fn create_provider(&self, mut op: Operation) -> Result<Chat, String> {
        let lease = tokio::select! { lease = self.0.registry.execution_lease(op.point.harness) => lease, _ = self.0.cancel.cancelled() => return Err("Engine is shutting down".into()) };
        let harness = self.0.registry.resolve(op.point.harness).map_err(err)?;
        let idle = self
            .0
            .sessions
            .session_status(&op.request.source_chat_id)
            .is_none_or(|s| matches!(s.status, SessionStatus::Idle | SessionStatus::Errored));
        match harness
            .fork_native(
                &op.point,
                zeron_harness::NativeForkControls {
                    execution_lease: Some(Arc::new(lease)),
                    interrupt: self.0.cancel.clone(),
                    timeout: Duration::from_secs(90),
                    source_idle: idle,
                },
            )
            .await
        {
            Ok(child) => {
                if child.session_id.is_empty()
                    || child.session_id == op.point.source_session_id
                    || child.cwd != op.point.cwd
                {
                    op.phase = Phase::Indeterminate;
                    op.error = Some("Provider returned an invalid native fork identity".into());
                    self.save(&op)?;
                    return Err(op.error.unwrap());
                }
                op.child = Some(child);
                op.phase = Phase::ProviderCreated;
                self.save(&op)?;
                self.publish(&mut op)?;
                Ok(op.chat)
            }
            Err(error) => {
                op.phase = if matches!(error, zeron_harness::NativeForkError::Indeterminate(_)) {
                    Phase::Indeterminate
                } else {
                    Phase::Failed
                };
                op.error = Some(error.to_string());
                self.save(&op)?;
                Err(error.to_string())
            }
        }
    }
    fn publish(&self, op: &mut Operation) -> Result<(), String> {
        let child = op.child.clone().ok_or("Native child identity is missing")?;
        let target = self.0.docs.open(&op.chat.id).map_err(err)?;
        let lineage = target.doc().native_fork_lineage().map_err(err)?;
        if lineage.as_ref().is_some_and(|lineage| {
            lineage.request_id != op.request.request_id || lineage.child != child
        }) {
            return Err("Destination belongs to another operation".into());
        }
        if let Some(row) = self.0.workspace.chat(&op.chat.id).map_err(err)? {
            if lineage.is_none()
                || row.harness_session_id.as_deref() != Some(child.session_id.as_str())
            {
                return Err(
                    "Destination was occupied while the provider fork was being created".into(),
                );
            }
            // The row may exist only in memory after a failed flush. Preserve
            // subsequent edits, but confirm persistence before acknowledging it.
            op.chat = row;
            self.0.workspace.flush().map_err(err)?;
            op.phase = Phase::Published;
            return self.save(op);
        }
        let existing = target.doc().read_entries().map_err(err)?;
        if lineage.is_none() && !existing.is_empty() {
            return Err("Destination document belongs to another operation".into());
        }
        // Claim the empty document before copying any rows, so interrupted
        // materialization is identifiable without comparing message text.
        target
            .doc()
            .set_native_fork_lineage(&NativeForkLineage {
                strategy: HistoryStrategy::NativeFork,
                request_id: op.request.request_id.clone(),
                source_chat_id: op.request.source_chat_id.clone(),
                source_message_id: op.request.source_message_id.clone(),
                point: op.point.clone(),
                child: child.clone(),
            })
            .map_err(err)?;
        for entry in &op.prefix {
            if existing.iter().any(|e| e.id == entry.id) {
                continue;
            }
            let mut entry = entry.clone();
            for part in &mut entry.parts {
                if let MessagePart::Input { resolved, .. } = part {
                    *resolved = true;
                }
                // Historical changes remain readable in their source conversation.
                // They are never new patches or pending approvals of this child.
                if let MessagePart::Tool {
                    diff, diff_stats, ..
                } = part
                {
                    *diff = None;
                    *diff_stats = None;
                }
            }
            target.doc().push_message(&entry).map_err(err)?;
        }
        let marker = format!("fork:{}", op.chat.id);
        if !existing.iter().any(|e| e.id == marker) {
            target
                .doc()
                .push_message(&SessionMessageEntry {
                    id: marker.clone(),
                    role: MessageRole::System,
                    parts: vec![MessagePart::Fork {
                        id: marker,
                        source_chat_id: op.request.source_chat_id.clone(),
                        source_title: "Conversation".into(),
                    }],
                    created_at: chrono::Utc::now().timestamp_millis(),
                    device_id: self.0.docs.device_id().into(),
                    status: Some(MessageStatus::Complete),
                    continuation_of: None,
                    duration_ms: None,
                    native_fork_point: None,
                })
                .map_err(err)?;
        }
        self.0.docs.persist_fork(&target).map_err(err)?;
        op.chat.harness_session_id = Some(child.session_id);
        op.chat.harness_session_cwd = Some(child.cwd);
        self.0.workspace.import_chat_row(&op.chat).map_err(err)?;
        self.0.workspace.flush().map_err(err)?;
        op.phase = Phase::Published;
        self.save(op)
    }
    pub async fn shutdown(&self) {
        self.0.tasks.close();
        self.0.cancel.cancel();
        self.0.tasks.wait().await;
    }
}
