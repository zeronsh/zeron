use anyhow::{Context, bail};
use clap::Subcommand;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Read, path::{Path, PathBuf}};

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    Current,
    /// Retain this exact live run until this attached process exits.
    /// Prints one JSON receipt; terminate and reap the child to release.
    Supervise,
    Read { chat_id: String },
    /// Publish selected imported terminal histories from the running owner controller.
    PublishImported { #[arg(required = true)] chat_ids: Vec<String> },
    Search {
        #[arg(long, required_unless_present="source_url")] query: Option<String>,
        #[arg(long, required_unless_present="query")] source_url: Option<String>,
        #[arg(long)] owner_id: Option<String>,
        #[arg(long, value_parser=clap::value_parser!(u8).range(1..=50))] limit: Option<u8>,
        #[arg(long)] cursor: Option<String>,
    },
    Fork { chat_id: Option<String>, #[arg(long)] command_id: Option<String> },
    Handoff {
        chat_id: Option<String>,
        #[arg(long)] prompt_file: PathBuf,
        #[arg(long, default_value="local", value_parser=["local","staging_snapshot","production_snapshot"])] database_environment: String,
        /// Transfer native OMP history only; keep the published sandbox checkout and omit local changes.
        #[arg(long)] session_only: bool,
        /// Persist this ID when recovering a lost acknowledgement. The default is the exact request digest.
        #[arg(long)] command_id: Option<String>,
        #[arg(long, requires="recover_sandbox_id")] recover_chat_id: Option<String>,
        #[arg(long, requires="recover_chat_id")] recover_sandbox_id: Option<String>,
    },
    Send {
        chat_id: String, text: String,
        #[arg(long)] from: Option<String>,
        #[arg(long)] command_id: Option<String>,
        #[arg(long)] wait: bool,
        #[arg(long, value_parser=clap::value_parser!(u64).range(0..=120000))] timeout: Option<u64>,
    },
    Reply {
        #[arg(long)] session: String,
        #[arg(long)] command: String,
        text: String,
        #[arg(long)] command_id: Option<String>,
        #[arg(long)] wait: bool,
    },
    Wait {
        #[arg(long)] session: String,
        #[arg(long)] thread: String,
        #[arg(long, value_parser=clap::value_parser!(u64).range(0..=120000))] timeout: Option<u64>,
    },
}

pub fn current(explicit: Option<String>) -> anyhow::Result<String> {
    explicit.or_else(|| std::env::var("COMET_SESSION_ID").ok())
        .map(|id| id.trim().to_owned()).filter(|id| !id.is_empty())
        .context("no source session: pass its ID or run inside a Crew agent session")
}
fn prompt_file(path: &Path) -> anyhow::Result<String> {
    let mut options = std::fs::OpenOptions::new(); options.read(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).context("cannot open prompt file")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() { bail!("prompt must be a regular file"); }
    let mut bytes = Vec::new(); file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 { bail!("prompt exceeds 1 MiB"); }
    let prompt = String::from_utf8(bytes).context("prompt must be UTF-8")?;
    if prompt.trim().is_empty() { bail!("prompt must not be empty"); }
    Ok(prompt)
}
fn print_json(value: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?); Ok(())
}
pub async fn connect(port: u16, data_dir: &Path) -> anyhow::Result<zeron_rpc::RpcClient> {
    zeron_rpc::connect_ipc(port, Some(data_dir)).await
        .with_context(|| format!("no Crew engine listening on 127.0.0.1:{port}; start the matching isolated engine"))
}
pub async fn run(command: SessionCommand, port: u16, data_dir: &Path) -> anyhow::Result<()> {
    if matches!(&command, SessionCommand::Current) { println!("{}", current(None)?); return Ok(()); }
    let client = connect(port, data_dir).await?;
    if let SessionCommand::PublishImported { chat_ids } = &command {
        for id in chat_ids {
            if !uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == *id) {
                bail!("imported session IDs must be canonical UUIDs");
            }
        }
        for id in chat_ids {
            print_json(&client.call("PublishImportedCrewSession", json!({"logicalSessionId": id})).await?)?;
        }
        return Ok(());
    }
    let (method, params) = match command {
        SessionCommand::Current | SessionCommand::PublishImported { .. } => unreachable!(),
        SessionCommand::Supervise => {
            let params = zeron_rpc::RetainSessionSupervisionParams {
                chat_id: current(None)?,
                token: std::env::var("COMET_SUPERVISION_TOKEN").ok()
                    .filter(|token| !token.is_empty())
                    .context("this run has no Crew supervision capability; start a fresh run in a supporting engine")?,
            };
            let mut retained = client.subscribe_checked(
                zeron_rpc::methods::RETAIN_SESSION_SUPERVISION,
                serde_json::to_value(params)?,
            ).await.context("Crew supervision is unavailable")?;
            let receipt = retained.recv().await
                .context("Crew supervision rejected or unsupported; no live run was retained")?;
            println!("{}", serde_json::to_string(&receipt)?);
            std::io::Write::flush(&mut std::io::stdout())?;
            while retained.recv().await.is_some() {}
            bail!("Crew supervision ended; the run or engine is no longer live");
        }
        SessionCommand::Read { chat_id } => {
            let mut stream = client.subscribe("WatchDocMessages", json!({"chatId":chat_id})).await?;
            let snapshot = stream.recv().await.context("transcript stream ended before its snapshot")?;
            return print_json(&snapshot);
        }
        SessionCommand::Search { query, source_url, owner_id, limit, cursor } =>
            ("SearchSessions", json!({"query":query,"sourceUrl":source_url,"ownerId":owner_id,"limit":limit,"cursor":cursor})),
        SessionCommand::Fork { chat_id, command_id } =>
            ("ForkSession", json!({"sourceChatId":current(chat_id)?,"commandId":command_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())})),
        SessionCommand::Handoff { chat_id, prompt_file: path, database_environment, session_only, command_id, recover_chat_id, recover_sandbox_id } => {
            let recovering = recover_chat_id.is_some();
            let mut params = json!({"sourceChatId":current(chat_id)?,"prompt":prompt_file(&path)?,
                "databaseEnvironment":database_environment,"sessionOnly":session_only,"recoverChatId":recover_chat_id,"recoverSandboxId":recover_sandbox_id});
            let command_id = command_id.unwrap_or_else(|| if recovering { String::new() }
                else { format!("native-{:x}", Sha256::digest(serde_json::to_vec(&params).expect("JSON input"))) });
            params["commandId"] = command_id.clone().into();
            let receipt = call_handoff(&client, params, session_only, recovering).await.with_context(|| format!(
                "native handoff command {command_id:?} was not retried automatically; inspect the durable target in Crew, then recover with its exact IDs (no standalone handoff fallback)"))?;
            return print_json(&receipt);
        }
        SessionCommand::Send { chat_id, text, from, command_id, wait, timeout } =>
            ("SendPeerMessage", json!({"sourceChatId":current(from)?,"targetChatId":chat_id,"text":text,
                "commandId":command_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),"wait":wait,"timeoutMs":timeout})),
        SessionCommand::Reply { session, command, text, command_id, wait } =>
            ("ReplyPeerMessage", json!({"sessionId":session,"commandId":command,"text":text,
                "sendCommandId":command_id.unwrap_or_else(|| format!("reply:{command}")),"wait":wait})),
        SessionCommand::Wait { session, thread, timeout } =>
            ("WaitPeerReply", json!({"sourceChatId":session,"threadId":thread,"timeoutMs":timeout})),
    };
    let command_id = params["sendCommandId"].as_str().or_else(|| params["commandId"].as_str()).map(str::to_owned);
    print_json(&client.call(method, params).await.with_context(|| format!("{method} failed; retain command ID {command_id:?} for an explicit retry; no automatic retry was attempted"))?)
}

fn handoff_method(session_only: bool, recovering: bool) -> &'static str {
    match (session_only, recovering) {
        (false, false) => "HandoffSessionToScaffold",
        (false, true) => "RecoverSessionHandoffToScaffold",
        (true, false) => "HandoffSessionOnlyToScaffold",
        (true, true) => "RecoverSessionOnlyHandoffToScaffold",
    }
}

async fn call_handoff(client: &zeron_rpc::RpcClient, params: Value, session_only: bool, recovering: bool) -> Result<Value, zeron_rpc::RpcError> {
    client.call(handoff_method(session_only, recovering), params).await
}

#[derive(Debug, Subcommand)]
pub enum ScaffoldCommand {
    /// List the current verified deployment's environments.
    List,
    /// Submit a typed ScaffoldEnvironmentControl JSON request (inspect/create/start/pause/resume/stop/attach/updateAgentRoute).
    Control { #[arg(long)] command_id: String, #[arg(long)] request_file: PathBuf },
    /// Read or reconcile an immutable operation; never replay an ambiguous create.
    Outcome { command_id: String, #[arg(long)] reconcile: bool },
}
pub async fn scaffold(command: ScaffoldCommand, port: u16, data_dir: &Path) -> anyhow::Result<()> {
    let client = connect(port, data_dir).await?;
    let value = match command {
        ScaffoldCommand::List => {
            let status = client.call("crew.status", json!({})).await?;
            client.call("scaffold.refresh", status.get("scope").cloned().context("verified scope unavailable")?).await?
        }
        ScaffoldCommand::Control { command_id, request_file } => {
            let control: zeron_proto::ScaffoldEnvironmentControl = serde_json::from_str(&prompt_file(&request_file)?)?;
            client.call("scaffold.control", json!({"commandId":command_id,"control":control})).await?
        }
        ScaffoldCommand::Outcome { command_id, reconcile } => client.call(
            if reconcile { "scaffold.reconcile" } else { "scaffold.outcome" }, json!({"commandId":command_id})).await?,
    };
    print_json(&value)
}
#[derive(Debug, clap::Args)]
pub struct InspectBackupArgs {
    #[arg(long)]
    source_backup_root: PathBuf,
    #[arg(long)]
    database_relative: PathBuf,
    #[arg(long = "native-history-root")]
    native_history_roots: Vec<PathBuf>,
    #[arg(long, required = true)]
    writers_stopped: bool,
}

pub fn inspect(args: InspectBackupArgs) -> anyhow::Result<()> {
    if !args.writers_stopped || !args.source_backup_root.is_absolute()
        || args.native_history_roots.iter().any(|root| !root.is_absolute()) {
        bail!("inspection requires absolute copied roots and stopped-writer confirmation");
    }
    print_json(&zeron_sync::crew_backup::inspect_inert_bootstraps(
        &args.source_backup_root, &args.database_relative, &args.native_history_roots)?)
}

#[derive(Debug, clap::Args)]
pub struct ExportBackupArgs {
    /// Original profile path, used only to preserve absolute attachment aliases.
    #[arg(long)]
    source_profile_root: PathBuf,
    /// Consistent copied profile; never the live original profile.
    #[arg(long)]
    source_backup_root: PathBuf,
    /// Selected project/account database path relative to the copied profile.
    #[arg(long)]
    database_relative: PathBuf,
    #[arg(long)]
    destination: PathBuf,
    /// Copied native journal roots; repeat for multiple roots.
    #[arg(long = "native-history-root")]
    native_history_roots: Vec<PathBuf>,
    /// Exact staged desktop source scope and migration receipt (never credentials).
    #[arg(long, requires = "source_storage_directory")]
    source_context: Option<PathBuf>,
    /// Copied legacy Edge workspace/session exports bound by source-context.
    #[arg(long, requires = "source_context")]
    source_storage_directory: Option<PathBuf>,
    /// Copied immutable attachment blobs named by the inventory.
    #[arg(long, requires = "source_context")]
    attachment_root: Option<PathBuf>,
    /// Coordinator-issued terminal receipts, sessions/<logical UUID>.json.
    #[arg(long, requires = "source_context")]
    terminal_authority_root: Option<PathBuf>,
    /// Return required source blobs without publishing an importer backup.
    #[arg(long, requires = "source_context")]
    inventory_attachments_only: bool,
    /// Read an existing controller credential and verify it directly; start no engine.
    #[arg(long, requires = "source_context")]
    identity_session: Option<PathBuf>,
    /// Confirm every source writer was stopped before the consistent copy.
    #[arg(long, required = true)]
    writers_stopped: bool,
}

impl ExportBackupArgs {
    pub fn offline(&self) -> bool { self.identity_session.is_some() }
}

pub async fn export(args: ExportBackupArgs, port: u16, data_dir: Option<&Path>) -> anyhow::Result<()> {
    if !args.writers_stopped { bail!("export requires an explicit stopped-writer confirmation"); }
    let native_roots = args.native_history_roots.into_iter().map(std::fs::canonicalize)
        .collect::<std::io::Result<Vec<_>>>()?;
    let context = args.source_context.map(|path| -> anyhow::Result<zeron_sync::crew_desktop_backup::DesktopSourceContext> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }).transpose()?;
    let storage = args.source_storage_directory.map(std::fs::canonicalize).transpose()?;
    let attachments = args.attachment_root.map(std::fs::canonicalize).transpose()?;
    let terminal_authority = args.terminal_authority_root.map(std::fs::canonicalize).transpose()?;
    if let Some(identity_path) = args.identity_session {
        let context = context.ok_or_else(|| anyhow::anyhow!("offline export requires source-context"))?;
        let storage = storage.ok_or_else(|| anyhow::anyhow!("offline export requires source-storage-directory"))?;
        let principal = verified_desktop_principal(&context, &identity_path).await?;
        let original = std::path::absolute(args.source_profile_root)?;
        let copied = args.source_backup_root.canonicalize()?;
        let destination = std::path::absolute(args.destination)?;
        let value = if args.inventory_attachments_only {
            serde_json::to_value(zeron_sync::crew_backup::inventory_desktop_crew_backup(
                &original, &copied, &args.database_relative, &destination, &native_roots, &principal, &context,
                &storage, attachments.as_deref(), terminal_authority.as_deref())?)?
        } else {
            serde_json::to_value(zeron_sync::crew_backup::export_desktop_crew_backup(
                &original, &copied, &args.database_relative, &destination, &native_roots, &principal, &context,
                &storage, attachments.as_deref(), terminal_authority.as_deref())?)?
        };
        return print_json(&value);
    }
    let params = json!({
        "sourceProfileRoot":std::path::absolute(args.source_profile_root)?,
        "sourceBackupRoot":args.source_backup_root.canonicalize()?,
        "databaseRelative":args.database_relative,
        "destination":std::path::absolute(args.destination)?,
        "nativeHistoryRoots":native_roots,
        "desktopSourceContext":context,
        "sourceStorageDirectory":storage,
        "attachmentRoot":attachments,
        "terminalAuthorityRoot":terminal_authority,
        "inventoryAttachmentsOnly":args.inventory_attachments_only,
        "writersStopped":true,
    });
    print_json(&connect(port, data_dir.context("online export requires the Crew profile")?).await?
        .call("ExportCrewBackup", params).await?)
}

async fn verified_desktop_principal(
    context: &zeron_sync::crew_desktop_backup::DesktopSourceContext, identity_path: &std::path::Path,
) -> anyhow::Result<zeron_proto::CollaborationPrincipal> {
    use zeron_sync::scaffold_auth::{ScaffoldAuth, ScaffoldConfig, load_session};
    let mut saved = load_session(identity_path)?.ok_or_else(|| anyhow::anyhow!("existing controller identity is required"))?;
    if saved.device.is_some() || saved.identity.user_id != context.owner_subject
        || saved.identity.project_scope != context.project_id
        || saved.identity.deployment_id.as_deref().is_some_and(|deployment| deployment != context.deployment_id) {
        bail!("offline migration requires the exact existing owner/controller identity");
    }
    let mut principal = zeron_proto::CollaborationPrincipal {
        subject: saved.identity.user_id.clone(), email: None, project_id: saved.identity.project_scope.clone(),
        deployment_id: Some(context.deployment_id.clone()), session_id: None,
        capabilities: vec![], unknown: Default::default(),
    };
    // Pin canonical staging authorities BEFORE sending a bearer anywhere. The
    // saved legacy null deployment is only an expected target hint in memory;
    // the current CP+Edge responses independently authenticate the projection.
    context.validate(&principal)?;
    let mut config = ScaffoldConfig::new(&context.target_resource, &context.target_origin, &context.project_id);
    config.deployment_id = principal.deployment_id.clone();
    saved.identity.deployment_id = principal.deployment_id.clone();
    let verified = ScaffoldAuth::new(config)?.verify(&saved).await?;
    principal.subject = verified.identity.user_id;
    principal.email = Some(verified.identity.email);
    principal.project_id = verified.identity.project_scope;
    principal.deployment_id = verified.identity.deployment_id;
    principal.capabilities = verified.identity.capabilities;
    context.validate(&principal)?;
    Ok(principal)
}

pub async fn import_offline(
    backup: PathBuf, destination: PathBuf, validate_only: bool, verify_only: bool,
    identity: PathBuf, source_context: PathBuf, device_id: String,
) -> anyhow::Result<()> {
    use zeron_sync::crew_import::{CrewBackupManifest, preflight_legacy_crew_backup, import_legacy_crew_backup, verify_legacy_crew_import};
    if device_id.trim().is_empty() || (validate_only && verify_only) { bail!("exact device ID and one import mode are required"); }
    let context: zeron_sync::crew_desktop_backup::DesktopSourceContext = serde_json::from_slice(&std::fs::read(source_context)?)?;
    let principal = verified_desktop_principal(&context, &identity).await?;
    let backup = backup.canonicalize()?;
    let destination = std::path::absolute(destination)?;
    let manifest: CrewBackupManifest = serde_json::from_slice(&std::fs::read(backup.join("crew-backup.json"))?)?;
    if manifest.desktop_source.as_ref().is_some_and(|proof| proof.migration_id != context.migration_id) {
        bail!("source context identifies a different migration");
    }
    if validate_only {
        let report = preflight_legacy_crew_backup(&backup, &destination, &principal, &device_id)?;
        print_json(&report)?;
        if report.get("valid").and_then(Value::as_bool) != Some(true) {
            bail!("backup validation failed; retain the JSON reconciliation report, no destination was written");
        }
        return Ok(());
    }
    let operation = if verify_only { verify_legacy_crew_import } else { import_legacy_crew_backup };
    print_json(&serde_json::to_value(operation(&backup, &destination, &principal, &device_id)?)?)
}

#[derive(clap::Args)]
pub struct ImportNativeProfileArgs {
    /// Consistent copy of the source profile directory (holding docs.sqlite3 and
    /// journals/), taken after its writer stopped; never the live profile.
    #[arg(long)]
    source_copy: PathBuf,
    /// The source profile's original uploads directory, granted read-only.
    #[arg(long)]
    source_uploads: Option<PathBuf>,
    /// Import only these chat ids (repeatable).
    #[arg(long = "chat")]
    chats: Vec<String>,
    /// Plan and validate only; write nothing.
    #[arg(long)]
    validate_only: bool,
    /// Confirm the source writer was stopped before the copy was taken.
    #[arg(long, required = true)]
    writers_stopped: bool,
}

pub async fn import_native_profile(args: ImportNativeProfileArgs, port: u16, data_dir: &Path) -> anyhow::Result<()> {
    let source_copy = std::path::absolute(&args.source_copy)?;
    let source_uploads = args.source_uploads.map(std::path::absolute).transpose()?;
    let chats = (!args.chats.is_empty()).then_some(args.chats);
    let report = connect(port, data_dir).await?.call("ImportNativeProfile", json!({
        "sourceCopy": source_copy, "sourceUploads": source_uploads, "chatIds": chats,
        "validateOnly": args.validate_only, "writersStopped": args.writers_stopped,
    })).await?;
    print_json(&report)?;
    if report.get("errors").and_then(Value::as_array).is_some_and(|errors| !errors.is_empty()) {
        bail!("native profile import reported errors; re-run after fixing them (completed chats are skipped)");
    }
    Ok(())
}

pub async fn import(backup: PathBuf, destination: PathBuf, validate_only: bool, verify_only: bool, port: u16, data_dir: &Path) -> anyhow::Result<()> {
    let backup = backup.canonicalize()?;
    let destination = std::path::absolute(destination)?;
    let report = connect(port, data_dir).await?.call("ImportCrewBackup", json!({"backup":backup,"destination":destination,"validateOnly":validate_only,"verifyOnly":verify_only})).await?;
    print_json(&report)?;
    if validate_only && report.get("valid").and_then(Value::as_bool) != Some(true) {
        bail!("backup validation failed; retain the JSON reconciliation report, no destination was written");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_only_uses_dedicated_creation_and_recovery_methods() {
        assert_eq!(handoff_method(true, false), "HandoffSessionOnlyToScaffold");
        assert_eq!(handoff_method(true, true), "RecoverSessionOnlyHandoffToScaffold");
        assert_eq!(handoff_method(false, false), "HandoffSessionToScaffold");
        assert_eq!(handoff_method(false, true), "RecoverSessionHandoffToScaffold");
    }
    #[tokio::test]
    async fn unsupported_handoff_modes_never_fall_back() {
        for (session_only, recovering) in [(false, true), (true, false), (true, true)] {
            let method = handoff_method(session_only, recovering);
            let (out, mut requests) = tokio::sync::mpsc::channel(4);
            let (responses, inbound) = tokio::sync::mpsc::channel(4);
            let client = zeron_rpc::RpcClient::new(out, inbound);
            let server = tokio::spawn(async move {
                let request: zeron_rpc::ClientFrame = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
                assert_eq!(request.method.as_deref(), Some(method));
                responses.send(serde_json::to_string(&zeron_rpc::ServerFrame {
                    id: request.id, err: Some(format!("unknown method: {method}")), ..Default::default()
                }).unwrap()).await.unwrap();
                assert!(requests.recv().await.is_none(), "unsupported mode must not fall back");
            });
            assert!(call_handoff(&client, json!({}), session_only, recovering).await.is_err());
            drop(client);
            server.await.unwrap();
        }
    }
    #[test]
    fn prompt_reader_bounds_input_without_consuming_it() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("prompt");
        std::fs::write(&path, "native task").unwrap();
        assert_eq!(prompt_file(&path).unwrap(), "native task");
        assert!(path.exists());
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(prompt_file(&path).is_err());
    }
}
