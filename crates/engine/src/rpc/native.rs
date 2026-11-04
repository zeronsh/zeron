//! Native CLI operations use the upstream durable command plane and transcript watch.
use super::EngineRpc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use zeron_doc::SessionCommandPayload;
use zeron_harness::CancellationToken;
use zeron_proto::{AgentRoute, Chat, HarnessId, RunRequest, ScaffoldDatabaseEnvironment};
use zeron_rpc::{RpcError, RpcReply};

// ponytail: serialize native branch preparation; per-source gates if contention matters.
static NATIVE_GATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
// A waiting reply's processed-ledger claim and command append are one admission.
static PEER_ADMISSION: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

type WaitKey = (String, String, String);
struct WaitOwner { id: String, after: Option<String>, sender: tokio::sync::oneshot::Sender<Value> }
static PEER_WAITS: LazyLock<parking_lot::Mutex<std::collections::HashMap<WaitKey, WaitOwner>>> =
    LazyLock::new(|| parking_lot::Mutex::new(Default::default()));
struct PeerWaitGuard { key: WaitKey, id: String, receiver: tokio::sync::oneshot::Receiver<Value> }
impl PeerWaitGuard {
    fn register(key: WaitKey, after: Option<&str>) -> Result<Self, RpcError> {
        let mut waits = PEER_WAITS.lock();
        if waits.contains_key(&key) { return Err(failed("A reply waiter already owns this session/thread")); }
        let id = crate::new_id();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        waits.insert(key.clone(), WaitOwner { id:id.clone(), after:after.map(str::to_owned), sender });
        Ok(Self { key, id, receiver })
    }
}
impl Drop for PeerWaitGuard {
    fn drop(&mut self) {
        let mut waits = PEER_WAITS.lock();
        if waits.get(&self.key).is_some_and(|owner| owner.id == self.id) { waits.remove(&self.key); }
    }
}

pub(super) fn handles(method: &str) -> bool {
    matches!(method, "ForkSession" | "HandoffSessionToScaffold" | "RecoverSessionHandoffToScaffold"
        | "HandoffSessionOnlyToScaffold" | "RecoverSessionOnlyHandoffToScaffold"
        | "SendPeerMessage" | "ReplyPeerMessage" | "WaitPeerReply" | "DeliverPeerMessage" | "ImportCrewBackup" | "ExportCrewBackup"
        | "VerifyImportedCrewSession" | "PublishImportedCrewSession" | "ImportNativeProfile")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportNativeProfileInput {
    source_copy: std::path::PathBuf,
    #[serde(default)]
    source_uploads: Option<std::path::PathBuf>,
    #[serde(default)]
    chat_ids: Option<std::collections::HashSet<String>>,
    #[serde(default)]
    validate_only: bool,
    writers_stopped: bool,
}
fn failed(error: impl std::fmt::Display) -> RpcError { RpcError::Failed(error.to_string()) }
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.trim().is_empty())
        .ok_or_else(|| RpcError::BadParams(format!("{key} is required")))
}
fn branch_id(command: &str) -> String {
    let hash = Sha256::digest(command.as_bytes());
    let mut bytes = [0; 16]; bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 15) | 0x80; bytes[8] = (bytes[8] & 63) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerifyImportedCrewSessionInput { logical_session_id: String }
impl VerifyImportedCrewSessionInput {
    fn parse(mut params: Value) -> Result<Self, RpcError> {
        if let Some(params) = params.as_object_mut() {
            params.remove("targetDeviceId");
            params.remove("__crewAuthority");
        }
        let input: Self = zeron_rpc::parse_params(params)?;
        if !uuid::Uuid::parse_str(&input.logical_session_id)
            .is_ok_and(|id| id.to_string() == input.logical_session_id) {
            return Err(RpcError::BadParams("logicalSessionId must be a canonical UUID".into()));
        }
        Ok(input)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyImportedCrewSessionResult {
    report: zeron_sync::crew_import::CrewImportReport,
    original_epoch_history_proof_json: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandoffInput {
    source_chat_id: String,
    command_id: String,
    prompt: String,
    database_environment: ScaffoldDatabaseEnvironment,
    #[serde(default)] session_only: bool,
    #[serde(default)] recover_chat_id: Option<String>,
    #[serde(default)] recover_sandbox_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct BranchIntent {
    source: Chat, native_id: String, cwd: String,
    transcript: Vec<zeron_doc::SessionMessageEntry>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerMessage {
    source_chat_id: String,
    #[serde(default)] target_chat_id: String,
    #[serde(default)] command_id: String,
    thread_id: String,
    #[serde(default)] reply_to: Option<String>,
    #[serde(default)] hop_count: u8,
    source_device_id: String,
    #[serde(default)] source_deployment_id: Option<String>,
    text: String,
}
const PEER_PREFIX: &str = "Crew peer message: ";
fn peer_prompt(message: &PeerMessage) -> Result<String, RpcError> {
    Ok(format!("{PEER_PREFIX}{}\n\n{}\n\nReply only if needed: comet session reply --session {} --command '{}' <text>",
        serde_json::to_string(message).map_err(failed)?, message.text, message.target_chat_id, message.command_id.replace('\'', "'\\''")))
}

fn validate_peer(message: &PeerMessage) -> Result<(), RpcError> {
    if message.source_chat_id == message.target_chat_id || message.text.trim().is_empty()
        || message.text.len() > 256 * 1024 || message.hop_count > 8
        || message.command_id.is_empty() || message.command_id.len() > 256
        || message.thread_id.is_empty() || message.thread_id.len() > 256
        || message.source_device_id.is_empty() || message.source_device_id.len() > 256
        || message.reply_to.as_ref().is_some_and(|id| id.is_empty() || id.len() > 256)
        || uuid::Uuid::parse_str(&message.source_chat_id).is_err()
        || uuid::Uuid::parse_str(&message.target_chat_id).is_err() {
        return Err(RpcError::BadParams("Invalid or out-of-bounds peer command".into()));
    }
    Ok(())
}
fn peer_from_payload(payload: &SessionCommandPayload) -> Option<PeerMessage> {
    let prompt = match payload {
        SessionCommandPayload::Run { request, .. } => &request.prompt,
        _ => return None,
    };
    serde_json::from_str(prompt.lines().next()?.strip_prefix(PEER_PREFIX)?).ok()
}
fn request_for(chat: &Chat, prompt: String) -> Result<RunRequest, RpcError> {
    let config = chat.config.as_ref().ok_or_else(|| failed("Session has no stored harness/model configuration"))?;
    if config.model.as_deref().is_none_or(str::is_empty) { return Err(failed("Session requires an explicit stored model")); }
    Ok(RunRequest { prompt, harness: Some(config.harness), model: config.model.clone(),
        reasoning: config.reasoning, model_options: config.model_options.clone(),
        cwd: chat.cwd.clone().or_else(|| chat.harness_session_cwd.clone()).ok_or_else(|| failed("Session has no working directory"))?,
        sandbox: config.sandbox, auto_approve: false, resume: None, attachments: vec![], worktree: None, mcp: None,
        access: config.access, plan: config.plan, env: Default::default() })
}

fn validate_recovered_native_input(original: HandoffInput, requested: &HandoffInput,
    preparation: &crate::crew::HandoffRequest) -> Result<HandoffInput, RpcError> {
    if original.source_chat_id != requested.source_chat_id || original.prompt != requested.prompt
        || original.database_environment != requested.database_environment || original.session_only != requested.session_only
        || (!requested.command_id.is_empty() && original.command_id != requested.command_id)
        || original.command_id != preparation.command_id || original.source_chat_id != preparation.source_session_id
        || original.prompt != preparation.prompt || original.database_environment != preparation.database_environment
        || original.session_only != preparation.session_only {
        return Err(failed("Recovery cannot change the caller command, source, prompt, database or transfer mode"));
    }
    // Selectors resolve allocation evidence, never become new immutable input.
    Ok(original)
}

fn claim_native_handoff(store: &zeron_sync::DocsStore, key: &str, input: &HandoffInput)
    -> Result<Option<crate::crew::HandoffReceipt>, RpcError> {
    let (bytes, outcome) = match store.crew_command(key).map_err(failed)? {
        Some(existing) => existing,
        None => {
            store.claim_crew_command(key, &serde_json::to_vec(input).map_err(failed)?).map_err(failed)?;
            store.crew_command(key).map_err(failed)?.ok_or_else(|| failed("Native handoff claim missing"))?
        }
    };
    // Older durable inputs omit sessionOnly=false. Validate every typed field
    // without rewriting or re-claiming their original immutable wire bytes.
    let saved: HandoffInput = serde_json::from_slice(&bytes).map_err(failed)?;
    if saved != *input { return Err(failed("Native handoff command has a different immutable input")); }
    let receipt = outcome.map(|bytes| serde_json::from_slice::<crate::crew::HandoffReceipt>(&bytes).map_err(failed)).transpose()?;
    if receipt.as_ref().is_some_and(|receipt| receipt.command_id != input.command_id) {
        return Err(failed("Native handoff receipt belongs to another caller command"));
    }
    Ok(receipt)
}

// Return true only for an already-durable admission of this exact original Run.
// Pending targets may contain an interrupted source copy, never unrelated work.
fn native_handoff_target(host: &crate::DocHost, workspace: &crate::WorkspaceHost, intent: &BranchIntent,
    input: &HandoffInput, receipt: &crate::crew::HandoffReceipt, expected_metadata: &zeron_proto::CrewChatMetadata) -> Result<bool, RpcError> {
    if receipt.command_id != input.command_id || receipt.environment.scope.session_id.as_deref() != Some(&receipt.chat_id)
        || expected_metadata.session_id != receipt.chat_id || expected_metadata.scope != receipt.environment.scope {
        return Err(failed("Native handoff receipt differs from its caller or target"));
    }
    let chat = workspace.chat(&receipt.chat_id).map_err(failed)?;
    let target = host.open(&receipt.chat_id).map_err(failed)?;
    let commands = target.doc().read_commands().map_err(failed)?;
    if let Some(command) = commands.iter().find(|command| command.id == input.command_id) {
        let mut request = request_for(&intent.source, input.prompt.clone())?;
        request.cwd = receipt.cwd.clone();
        let expected = SessionCommandPayload::Run { request, message_id: input.command_id.clone() };
        if command.payload != expected || command.issued_by != host.device_id() {
            return Err(failed("Native handoff command has a different immutable request"));
        }
        return Ok(true);
    }
    if !commands.is_empty() { return Err(failed("Pending native handoff target already contains other commands")); }
    let entries = target.doc().read_entries().map_err(failed)?;
    if let Some(chat) = chat {
        if chat.crew.as_ref().is_some_and(|metadata| metadata.owner_subject != expected_metadata.owner_subject
            || metadata.scope != expected_metadata.scope || metadata.session_id != expected_metadata.session_id) {
            return Err(failed("Pending native handoff target belongs to another owner or session scope"));
        }
        if receipt.attachment.attached_device_id.as_ref().is_some_and(|attached| !chat.device_id.is_empty()
            && &chat.device_id != attached && (chat.device_id != host.device_id() || chat.crew.is_some()
                || chat.harness_session_id.as_ref().is_some_and(|id| !id.is_empty()) || chat.config.is_some()
                || chat.source_context.is_some() || !entries.is_empty())) {
            return Err(failed("Pending native handoff target belongs to another hosting device"));
        }
        if chat.harness_session_id.as_ref().is_some_and(|id| !id.is_empty() && (id != &receipt.native_session_id
                || chat.harness_session_cwd.as_ref() != Some(&receipt.cwd) || chat.cwd.as_ref() != Some(&receipt.cwd)))
            || chat.harness_session_cwd.as_ref().is_some_and(|cwd| !cwd.is_empty() && cwd != &receipt.cwd)
            || chat.cwd.as_ref().is_some_and(|cwd| cwd != &receipt.cwd && cwd != "/workspace/ashler-platform")
        { return Err(failed("Pending native handoff target already has a different native association")); }
        if let Some(config) = &chat.config
            && Some(config) != intent.source.config.as_ref() {
            // Attach may have published only the route's default configuration.
            let source_config = intent.source.config.as_ref().ok_or_else(|| failed("Missing source model"))?;
            let mut route = AgentRoute::from_omp_model(source_config.model.as_deref().unwrap_or_default())
                .ok_or_else(|| failed("Native handoff requires an explicit provider-qualified OMP model"))?;
            route.account_id = source_config.model_options.get("crewAccountId").and_then(Value::as_str).map(str::to_owned);
            let mut model_options = serde_json::Map::new();
            if let Some(account) = &route.account_id { model_options.insert("crewAccountId".into(), json!(account)); }
            let published = zeron_proto::ChatConfig { harness: HarnessId::Omp, model: Some(route.omp_model()), reasoning: None,
                model_options, sandbox: zeron_proto::SandboxLevel::DangerFullAccess, access: Default::default(), plan: false };
            if config != &published {
                return Err(failed("Pending native handoff target already has different source configuration"));
            }
        }
        if chat.source_context.as_ref().is_some_and(|context| Some(context) != intent.source.source_context.as_ref()) {
            return Err(failed("Pending native handoff target already has different source context"));
        }
    }
    let count = intent.transcript.iter().rposition(|entry| entry.role == zeron_doc::MessageRole::Assistant
        && entry.status == Some(zeron_doc::MessageStatus::Complete)).map_or(0, |index| index + 1);
    let mut source = intent.transcript[..count].iter();
    for entry in entries {
        let saved = source.next().filter(|saved| saved.id == entry.id)
            .ok_or_else(|| failed("Pending native handoff target contains unrelated transcript progress"))?;
        if entry.role != saved.role || entry.created_at != saved.created_at || entry.device_id != saved.device_id
            || entry.status != saved.status || entry.continuation_of != saved.continuation_of || entry.duration_ms != saved.duration_ms
            || entry.parts.len() != saved.parts.len() || !entry.parts.iter().zip(&saved.parts).all(|(known, original)| match (known, original) {
                (zeron_doc::MessagePart::Input { id, request_id, questions, resolved: true, answers },
                    zeron_doc::MessagePart::Input { id: original_id, request_id: original_request,
                        questions: original_questions, answers: original_answers, .. }) =>
                    id == original_id && request_id == original_request && questions == original_questions
                        && answers == original_answers,
                (zeron_doc::MessagePart::Input { .. }, _) => false,
                _ => known == original,
            })
        { return Err(failed("Pending native handoff transcript differs from the immutable source snapshot")); }
    }
    Ok(false)
}

impl EngineRpc {
    fn native_controller(&self) -> Result<(), RpcError> {
        self.crew_services()?;
        if self.relay || self.auth()?.device_mode() { return Err(failed("Native branch preparation and backup import require local controller IPC")); }
        Ok(())
    }

    fn native_source(&self, id: &str) -> Result<BranchIntent, RpcError> {
        let source = self.crew_services()?.adopt_chat(&self.workspace, id).map_err(failed)?;
        self.sessions.ensure_execution_allowed(id, source.harness_session_id.as_deref()).map_err(failed)?;
        if source.device_id != self.doc_host.device_id() { return Err(failed("Native source must be hosted on this engine")); }
        if !matches!(source.config.as_ref().map(|config| config.harness), Some(HarnessId::Omp | HarnessId::ClaudeCode | HarnessId::Codex)) {
            return Err(failed("This harness has no supported native fork; no transcript-only fallback is permitted"));
        }
        request_for(&source, String::new())?;
        let native_id = source.harness_session_id.clone().filter(|id| !id.is_empty())
            .ok_or_else(|| failed("Source has no durable native identity"))?;
        let cwd = source.harness_session_cwd.clone().filter(|cwd| !cwd.is_empty())
            .ok_or_else(|| failed("Source has no durable native working directory"))?;
        if source.cwd.as_deref().is_some_and(|path| path != cwd
            && !std::fs::canonicalize(path).is_ok_and(|a| std::fs::canonicalize(&cwd).is_ok_and(|b| a == b))) {
            return Err(failed("Source working directory differs from its native journal"));
        }
        let mut transcript = self.doc_host.open(id).map_err(failed)?.doc().read_entries().map_err(failed)?;
        let count = transcript.iter().rposition(|entry| entry.role == zeron_doc::MessageRole::Assistant
            && entry.status == Some(zeron_doc::MessageStatus::Complete)).map_or(0, |index| index + 1);
        transcript.truncate(count);
        Ok(BranchIntent { source, native_id, cwd, transcript })
    }

    async fn prepare_native_branch(&self, key: &str, source_id: &str) -> Result<(BranchIntent, String, String), RpcError> {
        self.sessions.ensure_execution_allowed(source_id, None).map_err(failed)?;
        let store = self.doc_host.crew_store();
        let intent_key = format!("{key}:snapshot");
        let intent: BranchIntent = match store.crew_command(&intent_key).map_err(failed)? {
            Some((bytes, _)) => serde_json::from_slice(&bytes).map_err(failed)?,
            None => {
                let intent = self.native_source(source_id)?;
                store.claim_crew_command(&intent_key, &serde_json::to_vec(&intent).map_err(failed)?).map_err(failed)?;
                intent
            }
        };
        self.sessions.ensure_execution_allowed(source_id, Some(&intent.native_id)).map_err(failed)?;
        if intent.source.config.as_ref().map(|config| config.harness) != Some(HarnessId::Omp) {
            return Err(failed("Scaffold native transfer requires an OMP source"));
        }
        if intent.source.id != source_id { return Err(failed("Native snapshot source is immutable")); }
        let id = branch_id(key);
        let path = self.sessions.native_branch_path(&id).map_err(failed)?;
        let prepared = store.crew_command(&intent_key).map_err(failed)?.is_some_and(|(_, outcome)| outcome.is_some());
        if prepared && !path.is_file() {
            return Err(failed("Prepared native snapshot is missing; refusing to replace it with later history"));
        }
        let selector = self.sessions.native_launch_selector(&intent.native_id);
        let expected_cwd = intent.cwd.clone();
        let expected_id = id.clone();
        let path_string = path.to_string_lossy().into_owned();
        let scaffold_host = self.auth.as_ref().is_some_and(|auth| auth.device_mode());
        let captured = tokio::task::spawn_blocking(move || {
            if !path.exists() {
                zeron_harness::omp::fork_native_session(&selector, &path, &expected_id, scaffold_host).map_err(failed)?;
            }
            if !prepared {
                let blobs = zeron_harness::omp::native_session_blob_dir(&selector, scaffold_host).map_err(failed)?;
                crate::omp_session_artifact::prepare_native_branch_attachments(
                    &path, &expected_id, &expected_cwd, &blobs).map_err(failed)?;
            }
            let captured = crate::omp_session_artifact::capture_omp_file_for_session(
                &path.to_string_lossy(), &expected_cwd, &CancellationToken::new()).map_err(failed)?;
            if captured.native_session_id != expected_id { return Err(failed("Prepared branch identity changed")); }
            Ok::<_, RpcError>(json!({"nativeId":expected_id,"sha256":captured.sha256,"bytes":captured.byte_count}))
        }).await.map_err(failed)??;
        // A retry verifies the exact saved snapshot; it never recaptures a later source turn.
        store.finish_crew_command(&intent_key, &serde_json::to_vec(&captured).map_err(failed)?).map_err(failed)?;
        Ok((intent, id, path_string))
    }

    pub(super) async fn native_rpc(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            "ImportNativeProfile" => {
                self.native_controller()?;
                let (subject, scope) = self.crew_services()?.controller()?;
                if scope.session_id.is_some() {
                    return Err(failed("Native profile import requires owner controller authority"));
                }
                let input: ImportNativeProfileInput = serde_json::from_value(params)
                    .map_err(|error| RpcError::BadParams(error.to_string()))?;
                if !input.writers_stopped {
                    return Err(RpcError::BadParams("explicit stopped-writer confirmation required".into()));
                }
                let importer = crate::native_profile_import::NativeProfileImporter {
                    doc_host: self.doc_host.clone(),
                    workspace: self.workspace.clone(),
                    uploads: self.uploads.clone(),
                    target: crate::native_profile_import::NativeImportTarget {
                        owner_subject: subject.clone(),
                        project_id: scope.project_id.clone(),
                        deployment_id: scope.deployment_id.clone(),
                    },
                };
                let report = importer.run(crate::native_profile_import::NativeImportOptions {
                    source_copy: input.source_copy,
                    source_uploads: input.source_uploads,
                    chat_ids: input.chat_ids,
                    validate_only: input.validate_only,
                }).await.map_err(failed)?;
                let (current_subject, current_scope) = self.crew_services()?.controller()?;
                if current_subject != subject || current_scope != scope {
                    return Err(failed("Crew controller scope changed during native profile import"));
                }
                RpcReply::value(&report)
            }
            "VerifyImportedCrewSession" | "PublishImportedCrewSession" => {
                let (subject, scope) = self.crew_services()?.controller()?;
                if scope.session_id.is_some() {
                    return Err(failed("Imported profile verification requires owner controller authority"));
                }
                let input = VerifyImportedCrewSessionInput::parse(params)?;
                let principal = zeron_proto::CollaborationPrincipal {
                    subject: subject.clone(), email: self.auth()?.verified_owner_email(),
                    project_id: scope.project_id.clone(), deployment_id: scope.deployment_id.clone(),
                    session_id: None, capabilities: vec![], unknown: Default::default(),
                };
                let store = self.doc_host.crew_store();
                let device = self.doc_host.device_id().to_owned();
                let expected_device = device.clone();
                let report = tokio::task::spawn_blocking(move || {
                    let profile = store.profile_root().map_err(failed)?;
                    let report = zeron_sync::crew_import::verify_imported_profile(
                        &profile, &principal, &device, &input.logical_session_id).map_err(failed)?;
                    let proof_json = zeron_sync::crew_import::imported_original_epoch_proof_json(&profile, &report).map_err(failed)?;
                    Ok::<_, RpcError>((report, proof_json))
                }).await.map_err(failed)?;
                let (current_subject, current_scope) = self.crew_services()?.controller()?;
                if current_subject != subject || current_scope != scope
                    || self.doc_host.device_id() != expected_device {
                    return Err(failed("Crew controller scope changed during imported profile verification"));
                }
                let (report, original_epoch_history_proof_json) = report?;
                if method == "PublishImportedCrewSession" {
                    self.doc_host.publish_imported_terminal_history(&report).await.map_err(failed)?;
                    let (current_subject, current_scope) = self.crew_services()?.controller()?;
                    if current_subject != subject || current_scope != scope || self.doc_host.device_id() != expected_device {
                        return Err(failed("Crew controller scope changed during imported history publication"));
                    }
                }
                RpcReply::value(&VerifyImportedCrewSessionResult { report, original_epoch_history_proof_json })
            }
            "ImportCrewBackup" | "ExportCrewBackup" => {
                self.native_controller()?;
                let auth = self.auth()?;
                let scope = auth.verified_scope().ok_or_else(|| failed("Verified scope required"))?;
                let principal = zeron_proto::CollaborationPrincipal {
                    subject: auth.user_id().ok_or_else(|| failed("Verified owner required"))?,
                    email: auth.verified_owner_email(), project_id: scope.project_id,
                    deployment_id: scope.deployment_id, session_id: scope.session_id,
                    capabilities: vec![], unknown: Default::default(),
                };
                if method == "ExportCrewBackup" {
                    if params.get("writersStopped").and_then(Value::as_bool) != Some(true) {
                        return Err(RpcError::BadParams("explicit stopped-writer confirmation required".into()));
                    }
                    let original = std::path::PathBuf::from(string(&params, "sourceProfileRoot")?);
                    let copied = std::path::PathBuf::from(string(&params, "sourceBackupRoot")?);
                    let database = std::path::PathBuf::from(string(&params, "databaseRelative")?);
                    let destination = std::path::PathBuf::from(string(&params, "destination")?);
                    let native_roots: Vec<std::path::PathBuf> = serde_json::from_value(params.get("nativeHistoryRoots")
                        .cloned().unwrap_or_else(|| json!([]))).map_err(|_| RpcError::BadParams("nativeHistoryRoots must be paths".into()))?;
                    let context: Option<zeron_sync::crew_desktop_backup::DesktopSourceContext> = serde_json::from_value(
                        params.get("desktopSourceContext").cloned().unwrap_or(Value::Null)).map_err(|_| RpcError::BadParams("invalid desktop source context".into()))?;
                    let storage: Option<std::path::PathBuf> = serde_json::from_value(params.get("sourceStorageDirectory").cloned().unwrap_or(Value::Null))
                        .map_err(|_| RpcError::BadParams("sourceStorageDirectory must be a path".into()))?;
                    let attachments: Option<std::path::PathBuf> = serde_json::from_value(params.get("attachmentRoot").cloned().unwrap_or(Value::Null))
                        .map_err(|_| RpcError::BadParams("attachmentRoot must be a path".into()))?;
                    let terminal_authority: Option<std::path::PathBuf> = serde_json::from_value(params.get("terminalAuthorityRoot").cloned().unwrap_or(Value::Null))
                        .map_err(|_| RpcError::BadParams("terminalAuthorityRoot must be a path".into()))?;
                    let inventory_only: bool = serde_json::from_value(params.get("inventoryAttachmentsOnly").cloned().unwrap_or(Value::Bool(false)))
                        .map_err(|_| RpcError::BadParams("inventoryAttachmentsOnly must be a Boolean".into()))?;
                    if context.is_some() != storage.is_some()
                        || (context.is_none() && (attachments.is_some() || terminal_authority.is_some() || inventory_only)) {
                        return Err(RpcError::BadParams("desktop source context and storage are required together for witnessed export".into()));
                    }
                    if [&original, &copied, &destination].into_iter().chain(native_roots.iter())
                        .chain(storage.iter()).chain(attachments.iter()).chain(terminal_authority.iter()).any(|path| !path.is_absolute()) {
                        return Err(RpcError::BadParams("source and destination roots must be absolute paths".into()));
                    }
                    let manifest = tokio::task::spawn_blocking(move || -> Result<Value, zeron_sync::crew_import::CrewImportError> {
                        let manifest = if let (Some(context), Some(storage)) = (context, storage) {
                            if inventory_only {
                                return Ok(serde_json::to_value(zeron_sync::crew_backup::inventory_desktop_crew_backup(
                                    &original, &copied, &database, &destination, &native_roots, &principal, &context,
                                    &storage, attachments.as_deref(), terminal_authority.as_deref())?)?);
                            }
                            zeron_sync::crew_backup::export_desktop_crew_backup(&original, &copied, &database, &destination,
                                &native_roots, &principal, &context, &storage, attachments.as_deref(), terminal_authority.as_deref())?
                        } else {
                            zeron_sync::crew_backup::export_legacy_crew_backup(&original, &copied, &database, &destination, &native_roots, &principal)?
                        };
                        Ok(serde_json::to_value(manifest)?)
                    })
                        .await.map_err(failed)?.map_err(failed)?;
                    return RpcReply::value(&manifest);
                }
                let source = std::path::PathBuf::from(string(&params, "backup")?);
                let destination = std::path::PathBuf::from(string(&params, "destination")?);
                if !source.is_absolute() || !destination.is_absolute() { return Err(failed("Backup and new destination must be absolute paths")); }
                let device = self.doc_host.device_id().to_owned();
                let validate_only = match params.get("validateOnly") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    Some(_) => return Err(RpcError::BadParams("validateOnly must be a boolean".into())),
                };
                let verify_only = match params.get("verifyOnly") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    Some(_) => return Err(RpcError::BadParams("verifyOnly must be a boolean".into())),
                };
                if validate_only && verify_only { return Err(RpcError::BadParams("validation and destination readback are separate modes".into())); }
                let report = tokio::task::spawn_blocking(move || {
                    if validate_only {
                        zeron_sync::crew_import::preflight_legacy_crew_backup(&source, &destination, &principal, &device)
                    } else {
                        let import = if verify_only { zeron_sync::crew_import::verify_legacy_crew_import }
                            else { zeron_sync::crew_import::import_legacy_crew_backup };
                        import(&source, &destination, &principal, &device)
                            .and_then(|report| serde_json::to_value(report).map_err(Into::into))
                    }
                }).await.map_err(failed)?.map_err(failed)?;
                RpcReply::value(&report)
            }
            "ForkSession" => {
                self.native_controller()?;
                let _gate = NATIVE_GATE.lock().await;
                let source_id = string(&params, "sourceChatId")?;
                self.sessions.ensure_execution_allowed(source_id, None).map_err(failed)?;
                let command = string(&params, "commandId")?;
                if command.len() > 256 { return Err(RpcError::BadParams("commandId exceeds 256 bytes".into())); }
                let key = format!("native-fork:{command}");
                let store = self.doc_host.crew_store();
                store.claim_crew_command(&key, &serde_json::to_vec(&params).map_err(failed)?).map_err(failed)?;
                if let Some((_, Some(receipt))) = store.crew_command(&key).map_err(failed)? {
                    return RpcReply::value(&serde_json::from_slice::<Value>(&receipt).map_err(failed)?);
                }
                let seed_key = format!("{key}:source");
                let seed: BranchIntent = match store.crew_command(&seed_key).map_err(failed)? {
                    Some((bytes, _)) => serde_json::from_slice(&bytes).map_err(failed)?,
                    None => {
                        let seed = self.native_source(source_id)?;
                        store.claim_crew_command(&seed_key, &serde_json::to_vec(&seed).map_err(failed)?).map_err(failed)?;
                        seed
                    }
                };
                self.sessions.ensure_execution_allowed(source_id, Some(&seed.native_id)).map_err(failed)?;
                let native_fork_pending = seed.source.config.as_ref().map(|config| config.harness) != Some(HarnessId::Omp);
                let (intent, id) = if native_fork_pending {
                    let id = seed.native_id.clone();
                    (seed, id)
                } else {
                    let (intent, id, _) = self.prepare_native_branch(&key, source_id).await?;
                    (intent, id)
                };
                let mut chat = intent.source.clone();
                chat.id = branch_id(&format!("{key}:chat")); chat.parent_chat_id = Some(source_id.into());
                if native_fork_pending {
                    store.claim_crew_command(&format!("native-fork-source:{}", chat.id),
                        &serde_json::to_vec(&json!({"source":intent.native_id,"harness":chat.config.as_ref().map(|config| config.harness)})).map_err(failed)?)
                        .map_err(failed)?;
                }
                chat.title = None; chat.archived = false; chat.created_at = chrono::Utc::now();
                chat.last_message_at = None; chat.last_message_preview = None; chat.last_seen_at = None;
                chat.harness_session_id = Some(id); chat.harness_session_cwd = Some(intent.cwd.clone());
                chat.cwd = Some(intent.cwd); chat.room_gen = Some(2);
                chat.crew = Some(self.crew_services()?.session_metadata(&chat.id).map_err(failed)?);
                self.copy_native_transcript(&intent.transcript, &chat.id)?;
                self.workspace.import_chat_row(&chat).map_err(failed)?;
                let receipt = json!({"chatId":chat.id,"commandId":command,"nativeSessionId":chat.harness_session_id,"nativeForkPending":native_fork_pending});
                store.finish_crew_command(&key, &serde_json::to_vec(&receipt).map_err(failed)?).map_err(failed)?;
                RpcReply::value(&receipt)
            }
            "HandoffSessionToScaffold" | "RecoverSessionHandoffToScaffold"
            | "HandoffSessionOnlyToScaffold" | "RecoverSessionOnlyHandoffToScaffold" => {
                self.native_controller()?;
                let _gate = NATIVE_GATE.lock().await;
                let mut input: HandoffInput = zeron_rpc::parse_params(params)?;
                let session_only = matches!(method, "HandoffSessionOnlyToScaffold" | "RecoverSessionOnlyHandoffToScaffold");
                if input.session_only && !session_only {
                    return Err(failed("Session-only transfer requires its dedicated RPC method"));
                }
                input.session_only = session_only;
                let recovery = if matches!(method, "RecoverSessionHandoffToScaffold" | "RecoverSessionOnlyHandoffToScaffold") {
                    let chat = input.recover_chat_id.clone().filter(|id| !id.trim().is_empty()).ok_or_else(|| failed("Recovery requires chat ID"))?;
                    let sandbox = input.recover_sandbox_id.clone().filter(|id| !id.trim().is_empty()).ok_or_else(|| failed("Recovery requires sandbox ID"))?;
                    let (original, preparation) = self.recover_native_input(input).await?;
                    input = original;
                    Some((preparation, chat, sandbox))
                } else {
                    if input.recover_chat_id.is_some() || input.recover_sandbox_id.is_some() {
                        return Err(failed("Preserved targets require the recovery method"));
                    }
                    None
                };
                if input.prompt.trim().is_empty() || input.prompt.len() > 1024 * 1024 || input.command_id.is_empty() || input.command_id.len() > 256 {
                    return Err(RpcError::BadParams("Nonempty bounded prompt and immutable commandId required".into()));
                }
                let key = format!("native-handoff:{}", input.command_id);
                let store = self.doc_host.crew_store();
                if let Some(receipt) = claim_native_handoff(&store, &key, &input)? {
                    self.crew_services()?.admit_handoff(&receipt, || Ok(())).await?;
                    if recovery.is_some() { self.crew_services()?.cleanup_handoff_recovery(&input.command_id)?; }
                    return RpcReply::value(&receipt);
                }
                let (intent, _, selector) = self.prepare_native_branch(&key, &input.source_chat_id).await?;
                let config = intent.source.config.as_ref().ok_or_else(|| failed("Missing source model"))?;
                let mut route = AgentRoute::from_omp_model(config.model.as_deref().unwrap_or_default())
                    .ok_or_else(|| failed("Native handoff requires an explicit provider-qualified OMP model"))?;
                if let Some(account) = config.model_options.get("crewAccountId").and_then(Value::as_str) {
                    route.account_id = Some(account.into()); route.routing_mode = zeron_proto::AgentRoutingMode::Pinned;
                }
                let preparation = crate::crew::HandoffRequest {
                    command_id: input.command_id.clone(), source_session_id: input.source_chat_id.clone(),
                    native_session_id: selector, cwd: intent.cwd.clone(), prompt: input.prompt.clone(),
                    agent_route: route.clone(), database_environment: input.database_environment,
                    session_only: input.session_only,
                    recover_chat_id: input.recover_chat_id.clone(), recover_sandbox_id: input.recover_sandbox_id.clone(),
                };
                let explicit_recovery = recovery.is_some();
                let receipt = if let Some((original, chat, sandbox)) = recovery {
                    if preparation != original {
                        return Err(failed("Recovery saved native fork or immutable request changed"));
                    }
                    self.crew_services()?.recover_handoff(original, &chat, &sandbox, &CancellationToken::new()).await?
                } else {
                    self.crew_services()?.prepare_handoff(preparation, &CancellationToken::new()).await?
                };
                let expected_metadata = self.crew_services()?.session_metadata(&receipt.chat_id).map_err(failed)?;
                if native_handoff_target(&self.doc_host, &self.workspace, &intent, &input, &receipt, &expected_metadata)? {
                    self.crew_services()?.admit_handoff(&receipt, || Ok(())).await?;
                    store.finish_crew_command(&key, &serde_json::to_vec(&receipt).map_err(failed)?).map_err(failed)?;
                    if explicit_recovery { self.crew_services()?.cleanup_handoff_recovery(&input.command_id)?; }
                    return RpcReply::value(&receipt);
                }
                let mut chat = self.publish_crew_environment(receipt.attachment.clone(), Some(route)).await?;
                chat.config = intent.source.config.clone(); chat.cwd = Some(receipt.cwd.clone());
                chat.source_context = intent.source.source_context.clone();
                chat.harness_session_id = Some(receipt.native_session_id.clone());
                chat.harness_session_cwd = Some(receipt.cwd.clone());
                self.workspace.import_chat_row(&chat).map_err(failed)?;
                self.copy_native_transcript(&intent.transcript, &chat.id)?;
                let payload = SessionCommandPayload::Run { request: request_for(&chat, input.prompt)?, message_id: input.command_id.clone() };
                let host = self.doc_host.clone();
                let command = input.command_id.clone();
                let chat_id = chat.id.clone();
                self.crew_services()?.admit_handoff(&receipt, move || {
                    host.queue_command_named(&chat_id, payload, vec![], Some(&command), None).map_err(failed)
                }).await?;
                store.finish_crew_command(&key, &serde_json::to_vec(&receipt).map_err(failed)?).map_err(failed)?;
                if explicit_recovery { self.crew_services()?.cleanup_handoff_recovery(&input.command_id)?; }
                RpcReply::value(&receipt)
            }
            "DeliverPeerMessage" => {
                let rpc = self.clone();
                let receipt = tokio::task::spawn_blocking(move || rpc.deliver_native_peer(params)).await.map_err(failed)??;
                RpcReply::value(&receipt)
            }
            "SendPeerMessage" | "ReplyPeerMessage" => {
                self.native_peer(method, params).await
            }
            "WaitPeerReply" => {
                let source = string(&params, "sourceChatId")?;
                self.native_peer_source(source)?;
                let registration = PeerWaitGuard::register(self.native_wait_key(source, string(&params,"threadId")?)?, None)?;
                let result = self.wait_native_peer(registration, params.get("timeoutMs").and_then(Value::as_u64)).await?;
                RpcReply::value(&json!({"threadId":string(&params,"threadId")?,"reply":result}))
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }

    async fn recover_native_input(&self, requested: HandoffInput) -> Result<(HandoffInput, crate::crew::HandoffRequest), RpcError> {
        let chat = requested.recover_chat_id.as_deref().ok_or_else(|| failed("Recovery requires chat ID"))?;
        let sandbox = requested.recover_sandbox_id.as_deref().ok_or_else(|| failed("Recovery requires sandbox ID"))?;
        let preparation = self.crew_services()?.recover_handoff_request(chat, sandbox).await?;
        let (bytes, _) = self.doc_host.crew_store().crew_command(&format!("native-handoff:{}", preparation.command_id)).map_err(failed)?
            .ok_or_else(|| failed("Native recovery intent unavailable"))?;
        let original: HandoffInput = serde_json::from_slice(&bytes).map_err(failed)?;
        Ok((validate_recovered_native_input(original, &requested, &preparation)?, preparation))
    }

    fn copy_native_transcript(&self, entries: &[zeron_doc::SessionMessageEntry], target: &str) -> Result<(), RpcError> {
        let target = self.doc_host.open(target).map_err(failed)?;
        let existing = target.doc().read_entries().map_err(failed)?;
        if let Some(boundary) = entries.iter().rposition(|entry| entry.role == zeron_doc::MessageRole::Assistant
            && entry.status == Some(zeron_doc::MessageStatus::Complete)) {
            for entry in &entries[..=boundary] {
                if existing.iter().any(|known| known.id == entry.id) { continue; }
                let mut entry = entry.clone();
                for part in &mut entry.parts {
                    if let zeron_doc::MessagePart::Input { resolved, .. } = part { *resolved = true; }
                }
                target.doc().push_message(&entry).map_err(failed)?;
            }
        }
        self.doc_host.persist_fork(&target).map_err(failed)
    }

    fn native_peer_source(&self, id: &str) -> Result<Chat, RpcError> {
        let chat = self.crew_services()?.adopt_chat(&self.workspace, id).map_err(failed)?;
        if chat.device_id != self.doc_host.device_id() { return Err(failed("Peer source must be hosted by this engine")); }
        Ok(chat)
    }

    fn native_wait_key(&self, source: &str, thread: &str) -> Result<WaitKey, RpcError> {
        if thread.is_empty() || thread.len() > 256 { return Err(failed("Invalid peer thread ID")); }
        Ok((self.auth()?.verified_profile_key().ok_or_else(|| failed("Verified peer profile unavailable"))?,
            source.into(), thread.into()))
    }

    fn queue_native_peer(&self, message: &PeerMessage) -> Result<Value, RpcError> {
        let _admission = PEER_ADMISSION.lock();
        validate_peer(message)?;
        let target = self.crew_services()?.adopt_chat(&self.workspace, &message.target_chat_id).map_err(failed)?;
        if target.device_id != self.doc_host.device_id() { return Err(failed("Peer target is not owned by this engine")); }
        let handle = self.doc_host.open(&target.id).map_err(failed)?;
        let existing = handle.doc().read_commands().map_err(failed)?.into_iter().find(|entry| entry.id == message.command_id);
        let previously_admitted = existing.is_some();
        let payload = if let Some(existing) = existing {
            if peer_from_payload(&existing.payload).as_ref() != Some(message) {
                return Err(failed("Peer command ID has a different immutable sender or message"));
            }
            existing.payload
        } else {
            SessionCommandPayload::Run {
                request: request_for(&target, peer_prompt(message)?)?, message_id: message.command_id.clone(),
            }
        };
        let waiter = if message.reply_to.is_some() {
            let key = self.native_wait_key(&message.target_chat_id, &message.thread_id)?;
            let mut waits = PEER_WAITS.lock();
            if waits.get(&key).is_some_and(|owner| owner.after.as_deref().is_none_or(|id| message.reply_to.as_deref() == Some(id))) {
                waits.remove(&key)
            } else { None }
        } else { None };
        if waiter.is_some() && !previously_admitted {
            // A reply consumed by the waiting CLI is not a second agent prompt.
            // Claim before append, preserving upstream's no-replay crash rule.
            self.doc_host.crew_store().mark_processed(&message.command_id).map_err(failed)?;
            handle.doc().queue_command(&zeron_doc::SessionCommandEntry {
                id:message.command_id.clone(), payload, issued_by:self.doc_host.device_id().into(),
                issued_at:crate::now_ms(), based_on:None, expires_at:None,
                status:zeron_doc::SessionCommandStatus::Applied, resolution:Some("Delivered to waiting CLI".into()),
                accepted_at: None,
            }).map_err(failed)?;
            handle.write_user_message(&message.command_id, &peer_prompt(message)?, crate::now_ms()).map_err(failed)?;
            self.doc_host.persist_fork(&handle).map_err(failed)?;
        } else {
            self.doc_host.queue_command_named(&target.id, payload, vec![], Some(&message.command_id), None).map_err(failed)?;
        }
        if let Some(waiter) = waiter {
            let _ = waiter.sender.send(json!({"commandId":message.command_id,"sourceChatId":message.source_chat_id,"text":message.text}));
        }
        Ok(json!({"commandId":message.command_id,"threadId":message.thread_id,"status":"accepted","delivery":"pending"}))
    }

    fn deliver_native_peer(&self, params: Value) -> Result<Value, RpcError> {
        if !self.relay { return Err(failed("Directional peer admission is relay-only")); }
        let command = params.get("command").ok_or_else(|| failed("Missing peer command"))?;
        if command["kind"].as_str() != Some("peerMessage") { return Err(failed("Directional link accepts peer messages only")); }
        let mut message: PeerMessage = serde_json::from_value(command.clone()).map_err(failed)?;
        message.command_id = string(&params, "commandId")?.into();
        message.target_chat_id = string(&params, "chatId")?.into();
        validate_peer(&message)?;
        let authority = params.get("__crewAuthority").ok_or_else(|| failed("Missing verified peer authority"))?;
        let peer = authority.get("peer").ok_or_else(|| failed("Missing verified directional peer binding"))?;
        if peer["sourceSessionId"].as_str() != Some(message.source_chat_id.as_str())
            || peer["sourceDeviceId"].as_str() != Some(message.source_device_id.as_str())
            || peer["targetSessionId"].as_str() != Some(message.target_chat_id.as_str())
            || peer["targetDeviceId"].as_str() != Some(self.doc_host.device_id())
            || authority["deploymentId"].as_str() != message.source_deployment_id.as_deref()
            || authority.get("sessionId").and_then(Value::as_str).is_some_and(|id| id != message.source_chat_id)
            || authority.get("deviceId").and_then(Value::as_str).is_some_and(|id| id != message.source_device_id) {
            return Err(failed("Peer command does not match its verified directional authority"));
        }
        self.queue_native_peer(&message)
    }

    async fn native_peer(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        if self.relay { return Err(failed("Send/reply originate on local IPC; relay uses directional admission only")); }
        let command = params.get("sendCommandId").or_else(|| if method == "SendPeerMessage" { params.get("commandId") } else { None })
            .and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| {
                if method == "ReplyPeerMessage" { format!("reply:{}", params["commandId"].as_str().unwrap_or_default()) }
                else { crate::new_id() }
            });
        let text = string(&params, "text")?.to_owned();
        let scope = self.auth()?.verified_scope().ok_or_else(|| failed("Verified scope required"))?;
        let source_device = self.doc_host.device_id().to_owned();
        let (message, reply_device, target_deployment) = if method == "ReplyPeerMessage" {
            let source = string(&params, "sessionId")?;
            self.native_peer_source(source)?;
            let handle = self.doc_host.open(source).map_err(failed)?;
            let inbound = handle.doc().read_commands().map_err(failed)?.into_iter()
                .find(|entry| entry.id == string(&params, "commandId").unwrap_or_default())
                .and_then(|entry| peer_from_payload(&entry.payload)).ok_or_else(|| failed("Inbound peer command is unavailable"))?;
            if inbound.target_chat_id != source || inbound.hop_count >= 8 { return Err(failed("Invalid peer recipient or reply hop limit")); }
            (PeerMessage { source_chat_id: source.into(), target_chat_id: inbound.source_chat_id,
                command_id: command, thread_id: inbound.thread_id, reply_to: Some(inbound.command_id), hop_count: inbound.hop_count + 1,
                source_device_id:source_device, source_deployment_id:scope.deployment_id.clone(), text },
                Some(inbound.source_device_id), inbound.source_deployment_id)
        } else {
            (PeerMessage { source_chat_id: string(&params,"sourceChatId")?.into(), target_chat_id: string(&params,"targetChatId")?.into(),
                command_id: command.clone(), thread_id: command, reply_to: None, hop_count: 0,
                source_device_id:source_device, source_deployment_id:scope.deployment_id.clone(), text }, None, scope.deployment_id.clone())
        };
        validate_peer(&message)?;
        self.native_peer_source(&message.source_chat_id)?;
        if target_deployment != scope.deployment_id { return Err(failed("Peer reply crosses deployment scope")); }
        let timeout = params.get("timeoutMs").and_then(Value::as_u64);
        if timeout.is_some_and(|ms| ms > 120_000) { return Err(failed("Reply timeout may not exceed 120000ms")); }
        let registration = if params.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            Some(PeerWaitGuard::register(self.native_wait_key(&message.source_chat_id, &message.thread_id)?, Some(&message.command_id))?)
        } else { None };
        let store = self.doc_host.crew_store();
        let key = format!("native-peer:{}", message.command_id);
        store.claim_crew_command(&key, &serde_json::to_vec(&message).map_err(failed)?).map_err(failed)?;
        let mut result = if let Some((_, Some(bytes))) = store.crew_command(&key).map_err(failed)? {
            serde_json::from_slice::<Value>(&bytes).map_err(failed)?
        } else {
            let local = self.workspace.chat(&message.target_chat_id).map_err(failed)?
                .is_some_and(|chat| chat.device_id == self.doc_host.device_id());
            let receipt = if local {
                let rpc = self.clone(); let message = message.clone();
                tokio::task::spawn_blocking(move || rpc.queue_native_peer(&message)).await.map_err(failed)??
            } else {
                let links = self.links.as_ref().ok_or_else(|| failed("Directional peer relay is unavailable; retain the command ID"))?;
                let wire = json!({"chatId":message.target_chat_id,"commandId":message.command_id,"command":{
                    "kind":"peerMessage","text":message.text,"sourceChatId":message.source_chat_id,
                    "sourceDeploymentId":message.source_deployment_id,"sourceDeviceId":message.source_device_id,
                    "threadId":message.thread_id,"replyTo":message.reply_to,"hopCount":message.hop_count}});
                if let Some(device) = reply_device {
                    links.peer_device_call(&device, &message.target_chat_id, target_deployment.as_deref(), "DeliverPeerMessage", wire).await?
                } else {
                    links.peer_call(&message.target_chat_id, target_deployment.as_deref(), "DeliverPeerMessage", wire).await?
                }
            };
            if receipt["commandId"].as_str() != Some(message.command_id.as_str()) {
                return Err(failed("Peer admission outcome unknown; retry only the original command ID"));
            }
            store.finish_crew_command(&key, &serde_json::to_vec(&receipt).map_err(failed)?).map_err(failed)?;
            receipt
        };
        if let Some(registration) = registration {
            result["reply"] = self.wait_native_peer(registration, timeout).await?;
        }
        RpcReply::value(&result)
    }

    async fn wait_native_peer(&self, mut registration: PeerWaitGuard, timeout: Option<u64>) -> Result<Value, RpcError> {
        let timeout = timeout.unwrap_or(120_000);
        if timeout > 120_000 { return Err(RpcError::BadParams("Reply timeout may not exceed 120000ms".into())); }
        match tokio::time::timeout(std::time::Duration::from_millis(timeout), &mut registration.receiver).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(failed("Peer reply admission failed; inspect the preserved command")),
            Err(_) => Ok(Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_recovery_returns_original_native_input_and_rejects_changed_callers() {
        let original = HandoffInput { source_chat_id: "source".into(), command_id: "caller".into(), prompt: "original".into(),
            database_environment: ScaffoldDatabaseEnvironment::Local, session_only: false, recover_chat_id: None, recover_sandbox_id: None };
        let preparation = crate::crew::HandoffRequest { source_session_id: original.source_chat_id.clone(), command_id: original.command_id.clone(), prompt: original.prompt.clone(),
            native_session_id: "native".into(), cwd: "/repo".into(), agent_route: AgentRoute::from_omp_model("anthropic/claude-sonnet-4-5").unwrap(),
            database_environment: original.database_environment, session_only: original.session_only, recover_chat_id: None, recover_sandbox_id: None };
        for changed in ["none", "source", "prompt", "command", "mode", "database"] {
            let mut requested = original.clone();
            requested.recover_chat_id = Some("chat".into()); requested.recover_sandbox_id = Some("box".into());
            match changed {
                "source" => requested.source_chat_id = "other".into(),
                "prompt" => requested.prompt = "other".into(),
                "command" => requested.command_id = "other".into(),
                "mode" => requested.session_only = true,
                "database" => requested.database_environment = ScaffoldDatabaseEnvironment::StagingSnapshot,
                _ => {}
            }
            let recovered = validate_recovered_native_input(original.clone(), &requested, &preparation);
            if changed == "none" {
                assert_eq!(serde_json::to_value(recovered.unwrap()).unwrap(), serde_json::to_value(&original).unwrap());
            } else { assert!(recovered.is_err(), "{changed}"); }
        }
        let mut changed = preparation.clone(); changed.source_session_id = "other".into();
        assert!(validate_recovered_native_input(original.clone(), &original, &changed).is_err());
    }

    struct NativeReplayFixture {
        _dir: tempfile::TempDir,
        store: std::sync::Arc<zeron_sync::DocsStore>,
        workspace: crate::WorkspaceHost,
        host: crate::DocHost,
        intent: BranchIntent,
        input: HandoffInput,
        receipt: crate::crew::HandoffReceipt,
    }
    impl NativeReplayFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = std::sync::Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
            let workspace = crate::WorkspaceHost::open(store.clone(), crate::workspace_host::WorkspaceHostConfig {
                device_id: "controller".into(), device_name: "Host".into(), platform: "linux".into(),
                org_id: "project".into(), user_id: "owner".into(), edge: None, local_only: false,
                owner_registry: true,
            }).unwrap();
            workspace.create_chat("source", None, Some("controller"), None, Some("/source".into())).unwrap();
            workspace.create_chat("target", None, Some(&crate::scaffold::device_id_for("box", 7)), None, Some("/workspace/ashler-platform".into())).unwrap();
            let host = crate::DocHost::new(store.clone(), crate::DocHostConfig {
                device_id: "controller".into(), default_harness: HarnessId::Omp, edge: None,
            });
            host.set_workspace(workspace.clone());
            let mut source = workspace.chat("source").unwrap().unwrap();
            source.config = Some(zeron_proto::ChatConfig { harness: HarnessId::Omp,
                model: Some("anthropic/claude-sonnet-4-5".into()), reasoning: None,
                model_options: serde_json::from_value(json!({"source-setting":true})).unwrap(), sandbox: zeron_proto::SandboxLevel::DangerFullAccess,
                access: Default::default(), plan: false });
            workspace.import_chat_row(&source).unwrap();
            let source_doc = host.open("source").unwrap();
            for id in ["saved-first", "saved-last"] {
                source_doc.doc().push_message(&zeron_doc::SessionMessageEntry {
                    id: id.into(), role: zeron_doc::MessageRole::Assistant,
                    parts: vec![zeron_doc::MessagePart::Input { id: format!("part-{id}"), request_id: format!("input-{id}"),
                        questions: vec![], resolved: false, answers: None }],
                    created_at: 1, device_id: "controller".into(), status: Some(zeron_doc::MessageStatus::Complete),
                    continuation_of: None, duration_ms: None,
                }).unwrap();
            }
            host.persist_fork(&source_doc).unwrap();
            let intent = BranchIntent { source, native_id: "source-native".into(), cwd: "/source".into(),
                transcript: source_doc.doc().read_entries().unwrap() };
            let input = HandoffInput { source_chat_id: "source".into(), command_id: "caller".into(), prompt: "original prompt".into(),
                database_environment: ScaffoldDatabaseEnvironment::Local, session_only: false, recover_chat_id: None, recover_sandbox_id: None };
            let environment: zeron_proto::SessionEnvironment = serde_json::from_value(json!({
                "source": {"kind":"scaffold", "sandbox_id":"box", "lifecycle":"ready", "lifecycle_epoch":7},
                "ownerPrincipal":"owner@example.com", "scope":{"projectId":"project", "deploymentId":"deployment", "sessionId":"target"},
                "databaseEnvironment":"local"
            })).unwrap();
            let receipt = crate::crew::HandoffReceipt { chat_id: "target".into(), sandbox_id: "box".into(), command_id: "caller".into(),
                attachment: zeron_proto::ScaffoldEnvironmentControlResult { environment: environment.clone(), preparation_generation: None,
                    attached_device_id: Some(crate::scaffold::device_id_for("box", 7)), run_id: None, room_projection: None, control_grant: None,
                    handoff_native_session_id: None, handoff_cwd: None },
                environment, native_session_id: "recovered-fork-B".into(), cwd: "/workspace/crew-native-recovery".into() };
            Self { _dir: dir, store, workspace, host, intent, input, receipt }
        }
        fn metadata(&self) -> zeron_proto::CrewChatMetadata {
            zeron_proto::CrewChatMetadata { owner_subject: "owner".into(), scope: self.receipt.environment.scope.clone(),
                session_id: self.receipt.chat_id.clone(), environment: None, agent_route: None, original_epoch_history: None }
        }
        fn admit_and_progress(&self) {
            let mut original = self.intent.source.clone();
            original.cwd = Some(self.receipt.cwd.clone());
            let payload = SessionCommandPayload::Run { request: request_for(&original, self.input.prompt.clone()).unwrap(),
                message_id: self.input.command_id.clone() };
            self.host.queue_command_named("target", payload, vec![], Some("caller"), None).unwrap();
            let target = self.host.open("target").unwrap();
            target.doc().set_command_status("caller", zeron_doc::SessionCommandStatus::Cancelled, Some("Stopped")).unwrap();
            target.write_user_message("later-user", "do not erase progress", 2).unwrap();
            self.host.persist_fork(&target).unwrap();
            let mut chat = self.workspace.chat("target").unwrap().unwrap();
            chat.harness_session_id = Some("later-native-session".into());
            chat.harness_session_cwd = Some("/later/cwd".into());
            chat.cwd = Some("/later/cwd".into());
            chat.config = self.intent.source.config.clone();
            chat.config.as_mut().unwrap().model_options.insert("later-option".into(), json!(true));
            self.workspace.import_chat_row(&chat).unwrap();
        }
        fn reopened_host(&self) -> crate::DocHost {
            let host = crate::DocHost::new(self.store.clone(), crate::DocHostConfig {
                device_id: "controller".into(), default_harness: HarnessId::Omp, edge: None,
            });
            host.set_workspace(self.workspace.clone());
            host
        }

        async fn completed_rpc(&self) -> (crate::EngineCore, EngineRpc, tokio::task::JoinHandle<()>) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use zeron_sync::scaffold_auth::{save_session, ScaffoldConfig, ScaffoldIdentity, ScaffoldSession, DEFAULT_OAUTH_SCOPES};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let resource = origin.clone();
            let server = tokio::spawn(async move {
                for expected in ["GET /api/code-sandboxes/auth/session ", "GET /auth/session ", "GET /api/code-sandboxes/box "] {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut bytes = [0_u8; 8192];
                    let size = stream.read(&mut bytes).await.unwrap();
                    let request = String::from_utf8_lossy(&bytes[..size]);
                    assert!(request.starts_with(expected), "unexpected mutation or request: {request}");
                    let body = if expected == "GET /auth/session " {
                        json!({"userId":"owner", "email":"owner@example.com", "projectScope":"project", "deploymentId":"deployment",
                            "credential":"scaffold", "capabilities":["session.read"]})
                    } else if expected == "GET /api/code-sandboxes/auth/session " {
                        json!({"ok":true, "resource":resource, "actor":{"sub":"owner", "auth":"iap"},
                            "scopes":DEFAULT_OAUTH_SCOPES.split_whitespace().collect::<Vec<_>>()})
                    } else {
                        json!({"sandbox":{"id":"box", "lifecycleEpoch":7, "status":"ready", "kind":"remote_code", "runtimeProfile":"comet_remote",
                            "cometRuntimeProfile":{"version":"scaffold.comet-runtime.v1", "projectId":"project", "deploymentId":"deployment", "sessionId":"target", "sandboxId":"box", "databaseEnvironment":"local"},
                            "databaseEnvironment":"local", "ownerEmail":"owner@example.com", "createdAt":"2026-08-04T00:00:00Z", "updatedAt":"2026-08-04T00:01:00Z"}})
                    }.to_string();
                    stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            });
            let auth_dir = self._dir.path().join("auth");
            std::fs::create_dir_all(&auth_dir).unwrap();
            save_session(&auth_dir.join("scaffold-session.json"), &ScaffoldSession {
                access_token: "sc_rc_test".into(), resource: origin.clone(), edge_origin: origin.clone(), device: None,
                identity: ScaffoldIdentity { user_id: "owner".into(), email: "owner@example.com".into(), name: None,
                    project_scope: "project".into(), deployment_id: Some("deployment".into()), capabilities: vec!["session.read".into()] },
            }).unwrap();
            let mut config = crate::auth::AuthConfig::new(&origin, &auth_dir);
            config.scaffold = Some(ScaffoldConfig::new(&origin, &origin, "project"));
            let auth = crate::auth::Auth::detect(config).await;
            assert!(auth.state().is_signed_in());
            let runtime = crate::scaffold::ScaffoldRuntime::new(
                crate::scaffold::ScaffoldClient::new(&origin, "project", auth.for_profile_tokens()).unwrap(),
                &origin, std::sync::Arc::new(crate::scaffold::UnavailableDeviceJoinGrantProvider),
            ).with_deployment_id("deployment".into());
            let crew = crate::crew::CrewServices::new(auth.clone(), runtime, self.store.clone()).unwrap();
            let core = crate::EngineCore::assemble_with_profile(
                crate::EngineProfile::local(&self._dir.path().join("engine")).unwrap(),
                std::sync::Arc::new(crate::HarnessRegistry::new()), HarnessId::Omp, None,
            ).unwrap();
            let mut rpc = core.build_rpc().with_auth(auth).with_crew(crew);
            rpc.doc_host = self.reopened_host();
            rpc.workspace = self.workspace.clone();
            (core, rpc, server)
        }
    }

    #[tokio::test]
    async fn completed_native_receipt_replay_preserves_progress_without_source_preparation() {
        let fixture = NativeReplayFixture::new();
        fixture.admit_and_progress();
        let key = "native-handoff:caller";
        fixture.store.claim_crew_command(key, &serde_json::to_vec(&fixture.input).unwrap()).unwrap();
        fixture.store.finish_crew_command(key, &serde_json::to_vec(&fixture.receipt).unwrap()).unwrap();
        let host = fixture.reopened_host();
        let before_chat = fixture.workspace.chat("target").unwrap().unwrap();
        let before_entries = host.open("target").unwrap().doc().read_entries().unwrap();
        let replay = claim_native_handoff(&fixture.store, key, &fixture.input).unwrap().unwrap();
        let (core, rpc, server) = fixture.completed_rpc().await;
        rpc.native_rpc("HandoffSessionToScaffold", serde_json::to_value(&fixture.input).unwrap()).await.unwrap();
        server.await.unwrap();
        core.sessions.shutdown().await;
        assert_eq!(serde_json::to_value(replay).unwrap(), serde_json::to_value(&fixture.receipt).unwrap());
        assert!(fixture.store.crew_command("native-handoff:caller:snapshot").unwrap().is_none());
        assert_eq!(fixture.workspace.chat("target").unwrap().unwrap(), before_chat);
        assert_eq!(host.open("target").unwrap().doc().read_entries().unwrap(), before_entries);
        let commands = host.open("target").unwrap().doc().read_commands().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "caller");
        assert_eq!(commands[0].status, zeron_doc::SessionCommandStatus::Cancelled);
        let mut changed = fixture.input.clone(); changed.prompt = "different".into();
        assert!(claim_native_handoff(&fixture.store, key, &changed).is_err());
        let mut wrong = fixture.receipt.clone(); wrong.command_id = "another-caller".into();
        fixture.store.claim_crew_command("wrong-receipt", &serde_json::to_vec(&fixture.input).unwrap()).unwrap();
        fixture.store.finish_crew_command("wrong-receipt", &serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(claim_native_handoff(&fixture.store, "wrong-receipt", &fixture.input).is_err());
    }

    #[tokio::test]
    async fn legacy_native_handoff_claim_preserves_bytes_and_validates_every_input() {
        for completed in [false, true] {
            let fixture = NativeReplayFixture::new();
            let key = "native-handoff:caller";
            let mut legacy = serde_json::to_value(&fixture.input).unwrap();
            for field in ["sessionOnly", "recoverChatId", "recoverSandboxId"] {
                legacy.as_object_mut().unwrap().remove(field);
            }
            let original = serde_json::to_vec_pretty(&legacy).unwrap();
            fixture.store.claim_crew_command(key, &original).unwrap();
            if completed {
                fixture.store.finish_crew_command(key, &serde_json::to_vec(&fixture.receipt).unwrap()).unwrap();
            }
            let outcome = fixture.store.crew_command(key).unwrap().unwrap().1;
            let receipt = claim_native_handoff(&fixture.store, key, &fixture.input).unwrap();
            assert_eq!(receipt.is_some(), completed);
            assert_eq!(fixture.store.crew_command(key).unwrap().unwrap(), (original.clone(), outcome.clone()));
            for field in ["source", "prompt", "command", "database", "mode", "chat-selector", "sandbox-selector"] {
                let mut changed = fixture.input.clone();
                match field {
                    "source" => changed.source_chat_id = "other-source".into(),
                    "prompt" => changed.prompt = "changed prompt".into(),
                    "command" => changed.command_id = "other-command".into(),
                    "database" => changed.database_environment = ScaffoldDatabaseEnvironment::StagingSnapshot,
                    "mode" => changed.session_only = true,
                    "chat-selector" => changed.recover_chat_id = Some("other-chat".into()),
                    _ => changed.recover_sandbox_id = Some("other-sandbox".into()),
                }
                assert!(claim_native_handoff(&fixture.store, key, &changed).is_err(), "{field}");
                assert_eq!(fixture.store.crew_command(key).unwrap().unwrap(), (original.clone(), outcome.clone()));
            }
            let mut fresh = fixture.input.clone(); fresh.command_id = "new-caller".into();
            let fresh_key = "native-handoff:new-caller";
            assert!(claim_native_handoff(&fixture.store, fresh_key, &fresh).unwrap().is_none());
            assert_eq!(fixture.store.crew_command(fresh_key).unwrap().unwrap(), (serde_json::to_vec(&fresh).unwrap(), None));
        }
    }

    #[tokio::test]
    async fn interrupted_native_outcome_adopts_exact_original_admission_without_resetting_progress() {
        let fixture = NativeReplayFixture::new();
        let key = "native-handoff:caller";
        assert!(claim_native_handoff(&fixture.store, key, &fixture.input).unwrap().is_none());
        fixture.admit_and_progress();
        let host = fixture.reopened_host();
        let before_chat = fixture.workspace.chat("target").unwrap().unwrap();
        let before_entries = host.open("target").unwrap().doc().read_entries().unwrap();
        assert!(native_handoff_target(&host, &fixture.workspace, &fixture.intent, &fixture.input, &fixture.receipt, &fixture.metadata()).unwrap());
        fixture.store.finish_crew_command(key, &serde_json::to_vec(&fixture.receipt).unwrap()).unwrap();
        for changed in ["prompt", "message", "config", "cwd"] {
            let mut input = fixture.input.clone();
            let mut intent = fixture.intent.clone();
            let mut receipt = fixture.receipt.clone();
            match changed {
                "prompt" => input.prompt = "changed".into(),
                "message" => input.command_id = "other".into(),
                "config" => intent.source.config.as_mut().unwrap().model = Some("openai/other".into()),
                _ => receipt.cwd = "/different".into(),
            }
            assert!(native_handoff_target(&host, &fixture.workspace, &intent, &input, &receipt, &fixture.metadata()).is_err(), "{changed}");
        }
        assert_eq!(fixture.workspace.chat("target").unwrap().unwrap(), before_chat);
        assert_eq!(host.open("target").unwrap().doc().read_entries().unwrap(), before_entries);
        let commands = host.open("target").unwrap().doc().read_commands().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].status, zeron_doc::SessionCommandStatus::Cancelled);
    }

    #[tokio::test]
    async fn pending_native_target_rejects_unrelated_association_and_progress_but_allows_exact_partial_copy() {
        for scenario in ["partial", "attach-default", "stale-fork", "private-path", "unrelated", "wrong-cwd", "missing-native-cwd",
            "extra-entry", "changed-entry", "missing-prefix", "config", "command"] {
            let fixture = NativeReplayFixture::new();
            let mut chat = fixture.workspace.chat("target").unwrap().unwrap();
            chat.harness_session_id = Some(match scenario {
                "stale-fork" => "saved-fork-A".into(),
                "private-path" => "/native/recovered/journal.jsonl".into(),
                "unrelated" => "unrelated-native".into(),
                _ => fixture.receipt.native_session_id.clone(),
            });
            chat.harness_session_cwd = Some(fixture.receipt.cwd.clone());
            chat.cwd = Some(fixture.receipt.cwd.clone());
            chat.config = fixture.intent.source.config.clone();
            if scenario == "config" { chat.config.as_mut().unwrap().model = Some("openai/other".into()); }
            if scenario == "attach-default" { chat.config.as_mut().unwrap().model_options.clear(); }
            if scenario == "wrong-cwd" { chat.cwd = Some("/user/progress".into()); }
            if scenario == "missing-native-cwd" { chat.harness_session_cwd = None; }
            fixture.workspace.import_chat_row(&chat).unwrap();
            let target = fixture.host.open("target").unwrap();
            let mut entry = fixture.intent.transcript[usize::from(scenario == "missing-prefix")].clone();
            for part in &mut entry.parts {
                if let zeron_doc::MessagePart::Input { resolved, .. } = part { *resolved = true; }
            }
            if scenario == "changed-entry" { entry.created_at += 1; }
            target.doc().push_message(&entry).unwrap();
            if scenario == "extra-entry" { target.write_user_message("unrelated-user", "new work", 3).unwrap(); }
            fixture.host.persist_fork(&target).unwrap();
            if scenario == "command" {
                fixture.host.queue_command_named("target", SessionCommandPayload::Steer {
                    prompt: "unrelated work".into(), message_id: None,
                }, vec![], Some("other-command"), None).unwrap();
            }
            let host = fixture.reopened_host();
            let before_entries = host.open("target").unwrap().doc().read_entries().unwrap();
            let guarded = native_handoff_target(&host, &fixture.workspace, &fixture.intent, &fixture.input, &fixture.receipt, &fixture.metadata());
            if matches!(scenario, "partial" | "attach-default") { assert!(!guarded.unwrap()); }
            else { assert!(guarded.is_err(), "{scenario}"); }
            assert_eq!(fixture.workspace.chat("target").unwrap().unwrap(), chat);
            assert_eq!(host.open("target").unwrap().doc().read_entries().unwrap(), before_entries);
            assert_eq!(host.open("source").unwrap().doc().read_entries().unwrap(), fixture.intent.transcript);
        }
    }

    #[tokio::test]
    async fn pending_native_target_rejects_foreign_owner_scope_session_and_host_without_mutation() {
        for scenario in ["owner", "scope", "session", "host", "active-local-host", "placeholder", "admitted-foreign"] {
            let fixture = NativeReplayFixture::new();
            if scenario == "admitted-foreign" { fixture.admit_and_progress(); }
            let mut chat = fixture.workspace.chat("target").unwrap().unwrap();
            chat.device_id = fixture.receipt.attachment.attached_device_id.clone().unwrap();
            chat.crew = Some(fixture.metadata());
            match scenario {
                "owner" | "admitted-foreign" => chat.crew.as_mut().unwrap().owner_subject = "another-owner".into(),
                "scope" => chat.crew.as_mut().unwrap().scope.project_id = "another-project".into(),
                "session" => chat.crew.as_mut().unwrap().session_id = "another-session".into(),
                "host" => chat.device_id = "another-host".into(),
                "active-local-host" => { chat.crew = None; chat.device_id = "controller".into(); chat.config = fixture.intent.source.config.clone(); }
                _ => { chat.crew = None; chat.device_id = "controller".into(); }
            }
            fixture.workspace.import_chat_row(&chat).unwrap();
            let host = fixture.reopened_host();
            let entries = host.open("target").unwrap().doc().read_entries().unwrap();
            let guarded = native_handoff_target(&host, &fixture.workspace, &fixture.intent, &fixture.input, &fixture.receipt, &fixture.metadata());
            match scenario {
                "placeholder" => assert!(!guarded.unwrap()),
                "admitted-foreign" => assert!(guarded.unwrap()),
                _ => assert!(guarded.is_err(), "{scenario}"),
            }
            assert_eq!(fixture.workspace.chat("target").unwrap().unwrap(), chat);
            assert_eq!(host.open("target").unwrap().doc().read_entries().unwrap(), entries);
        }
    }


    #[tokio::test]
    async fn imported_profile_verification_authenticates_before_profile_lookup() {
        use zeron_rpc::RpcService;
        let temp = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(temp.path()).unwrap(),
            std::sync::Arc::new(crate::HarnessRegistry::new()), HarnessId::Omp, None,
        ).unwrap();
        for rpc in [core.build_rpc(), core.build_rpc().for_relay()] {
            for params in [json!({"logicalSessionId": "53a67c53-cbc2-4a86-bffd-4c9f5c735590"}),
                json!({"logicalSessionId": "invalid", "profileRoot": "/caller/path"})] {
                for method in ["VerifyImportedCrewSession", "PublishImportedCrewSession"] {
                    let error = match rpc.handle(method, params.clone()).await {
                        Err(error) => error,
                        Ok(_) => panic!("unauthenticated imported profile operation succeeded"),
                    };
                    assert!(error.to_string().contains("Crew sign-in required"));
                }
            }
        }
        assert!(core.rpc_service().handle("VerifyImportedCrewSession", json!({
            "logicalSessionId": "53a67c53-cbc2-4a86-bffd-4c9f5c735590",
            "__crewAuthority": {"subject": "owner"}
        })).await.is_err());
        core.sessions.shutdown().await;
    }

    #[test]
    fn imported_profile_verification_has_a_fixed_session_only_route() {
        let id = "53a67c53-cbc2-4a86-bffd-4c9f5c735590";
        assert!(handles("VerifyImportedCrewSession"));
        assert!(super::super::forwardable("VerifyImportedCrewSession"));
        assert!(handles("PublishImportedCrewSession"));
        assert!(super::super::forwardable("PublishImportedCrewSession"));
        assert!(VerifyImportedCrewSessionInput::parse(json!({"logicalSessionId": id})).is_ok());
        assert!(VerifyImportedCrewSessionInput::parse(json!({"logicalSessionId": id,
            "targetDeviceId": "host", "__crewAuthority": {}})).is_ok());
        for extra in ["backup", "source", "destination", "profileRoot", "principal", "report", "chatId"] {
            let mut params = json!({"logicalSessionId": id});
            params[extra] = json!("caller-controlled");
            assert!(VerifyImportedCrewSessionInput::parse(params).is_err());
        }
        for invalid in ["", "not-a-uuid", "53A67C53-CBC2-4A86-BFFD-4C9F5C735590", "53a67c53cbc24a86bffd4c9f5c735590"] {
            assert!(VerifyImportedCrewSessionInput::parse(json!({"logicalSessionId": invalid})).is_err());
        }
    }
    #[test]
    fn immutable_native_branch_and_peer_admission_identity() {
        assert_eq!(branch_id("caller-1"), branch_id("caller-1"));
        assert_ne!(branch_id("caller-1"), branch_id("caller-2"));
        let message = PeerMessage { source_chat_id:"source".into(),target_chat_id:"target".into(),
            command_id:"caller-1".into(),thread_id:"caller-1".into(),reply_to:None,hop_count:0,
            source_device_id:"device".into(),source_deployment_id:Some("canary".into()),text:"Do work".into() };
        let prompt = peer_prompt(&message).unwrap();
        let parsed: PeerMessage = serde_json::from_str(prompt.lines().next().unwrap().strip_prefix(PEER_PREFIX).unwrap()).unwrap();
        assert_eq!(parsed.command_id,"caller-1");
        assert!(prompt.contains("--session target --command 'caller-1'"));
    }

    #[tokio::test]
    async fn restart_admission_retains_caller_id_and_stopped_outcome() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let config = DocHostConfig { device_id:"controller".into(), default_harness:HarnessId::Mock, edge:None };
        let host = DocHost::new(store.clone(), config.clone());
        let payload = SessionCommandPayload::Steer { prompt:"remote task".into(), message_id:Some("caller".into()) };
        assert_eq!(host.queue_command_named("target", payload.clone(), vec![], Some("caller"), None).unwrap(), "caller");
        let handle = host.open("target").unwrap();
        handle.doc().set_command_status("caller", zeron_doc::SessionCommandStatus::Cancelled, Some("Stopped")).unwrap();
        host.persist_fork(&handle).unwrap();
        assert!(store.mark_processed("caller").unwrap());
        let reopened = DocHost::new(store.clone(), config);
        assert_eq!(reopened.queue_command_named("target", payload, vec![], Some("caller"), None).unwrap(), "caller");
        let entries = reopened.open("target").unwrap().doc().read_commands().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, zeron_doc::SessionCommandStatus::Cancelled);
        assert!(!store.mark_processed("caller").unwrap());
        assert!(reopened.queue_command_named("target",
            SessionCommandPayload::Steer { prompt:"different task".into(), message_id:Some("caller".into()) },
            vec![], Some("caller"), None).is_err());
    }

    #[tokio::test]
    async fn targeted_interrupt_rejects_stale_admission_and_preserves_retries() {
        use crate::doc_host::{DocHost, DocHostConfig};
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(store, DocHostConfig { device_id:"controller".into(), default_harness:HarnessId::Mock, edge:None });
        let handle = host.open("target").unwrap();
        handle.write_user_message("first", "first turn", 1).unwrap();
        let stop = SessionCommandPayload::Interrupt {};
        host.queue_command_named("target", stop.clone(), vec![], Some("stop-first"), Some("first")).unwrap();
        handle.write_user_message("second", "replacement turn", 2).unwrap();
        assert!(host.queue_command_named("target", stop.clone(), vec![], Some("stale"), Some("first")).is_err());
        assert_eq!(host.queue_command_named("target", stop.clone(), vec![], Some("stop-first"), Some("first")).unwrap(), "stop-first");
        assert!(host.queue_command_named("target", stop, vec![], Some("stop-first"), Some("second")).is_err());
        assert!(!handle.doc().read_commands().unwrap().iter().any(|entry| entry.id == "stale"));
    }

    #[tokio::test]
    async fn immediate_reply_is_reserved_once_and_old_wait_cannot_remove_new_wait() {
        let key = ("test-profile".into(), "source".into(), crate::new_id());
        let mut first = PeerWaitGuard::register(key.clone(), Some("request")).unwrap();
        assert!(PeerWaitGuard::register(key.clone(), None).is_err());
        let owner = PEER_WAITS.lock().remove(&key).unwrap();
        owner.sender.send(json!({"commandId":"reply:request","text":"answer"})).unwrap();
        assert_eq!((&mut first.receiver).await.unwrap()["text"], "answer");
        let second = PeerWaitGuard::register(key.clone(), None).unwrap();
        drop(first);
        assert!(PEER_WAITS.lock().contains_key(&key));
        drop(second);
        assert!(!PEER_WAITS.lock().contains_key(&key));
    }
}
