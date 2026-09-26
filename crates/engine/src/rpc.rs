//! EngineRpc — the engine-side `RpcService`: sessions + docs + the workspace-doc
//! entity surface.
//!
//! Methods (feature-inventory §2):
//! - `ListHarnesses` → `[HarnessDescriptor]`
//! - `ListModels {harness}` → `[Model]`
//! - `QueueCommand {chatId, command}` → `{commandId}` (durable doc command)
//! - `WatchDocMessages {chatId}` → stream of joined `SessionMessageEntry[]`,
//!   re-emitted on every doc change
//! - `WatchChats` / `WatchDevices` → streams of the workspace doc's entity rows
//! - `WatchSessions` → stream of `Session[]`: this engine's live statuses merged with
//!   remote devices' workspace session rows
//! - `Mutate {op, …}` → `{ok}` — workspace entity mutations (createChat, renameChat,
//!   setChatArchived, deleteChat, renameDevice, markChatSeen)
//! - `EngineInfo` → `{deviceId, workspaceScope}` — this runtime's fixed identity
//!   and data boundary (never forwarded)
//! - `LocalDevice` → `{deviceId}` — legacy engine identity (never forwarded)
//! - AuthRpc (feature-inventory §2): `AuthStatus` (stream), `SignIn`/`SignInHeadless` →
//!   `{url}`, `CompleteSignIn {code}`, `SignOut`, `ListOrgs`, `CreateOrg {name}`,
//!   `SelectOrg {organizationId}`
//! - Repos (§3.5): `ListRepos`, `AddRepo {path}`, `CloneRepo {url}`,
//!   `CreateRepo {name}`, `ListBranches {repoPath}` (default branch first),
//!   `ListFolders {path?}`, `CreateWorktree {repoPath, branch}`, `DeleteWorktree
//!   {repoPath, worktreePath}`; `WatchCheckoutDiffs` → stream of `CheckoutDiff[]`
//! - Workspace files: lazy directory listing, recursive path search, bounded text
//!   reads, hash-guarded writes, and a checkout-scoped filesystem change stream.
//! - Terminals (§3.4): `OpenTerminal {chatId, cols, rows, cwd?}` → `TerminalSession`,
//!   `SubscribeTerminal {terminalId, afterSeq?}` → stream of `TerminalEvent`
//!   (replay then live tail), `WriteTerminal {terminalId, data}`, `ResizeTerminal`,
//!   `CloseTerminal`. M5 is single-user local: per-user owner checks land with
//!   real multi-account auth in M6.
//! - Agent accounts (§3.7): `ListAgentAccounts {forceUsage?}` →
//!   `AgentAccountsSnapshot`, `ActivateAgentAccount`/`ForgetAgentAccount`
//!   `{harness, accountId}` → snapshot, `StartAgentLogin {harness}` →
//!   `{loginId, url, mode}`, `CompleteAgentLogin {loginId, code}` → snapshot,
//!   `PollAgentLogin {loginId}`, `CancelAgentLogin {loginId}`.
//! - Uploads (§3.7): `UploadChunk {uploadId, data, seq?}`,
//!   `UploadCommit {uploadId, fileName}` → `{path}`,
//!   `ReadAttachmentChunk {path, offset}` → `{name, mimeType, data, nextOffset,
//!   done}` (path-jailed to the uploads dir + workspace-known chat cwds).
//!
//! ## Device-addressed routing (`targetDeviceId`, feature-inventory §2.1)
//!
//! ControlRpc methods are relay-forwardable: params may carry `targetDeviceId`. When it
//! names another device, the call is forwarded verbatim over that device's relay DO via
//! the [`LinkCache`] — the remote engine sees its own id and handles locally, so the
//! forward can never loop. Streaming methods are proxied by re-subscribing remotely and
//! piping items. To make another method device-addressable, nothing per-method is needed
//! beyond listing it in [`forwardable`] (and [`is_stream_method`] if it streams);
//! handlers stay transport-agnostic. This includes the workspace file surface,
//! whose checkout always lives on the routed target device.

use async_trait::async_trait;
use base64::Engine as _;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::watch;

use zeron_doc::{MessagePart, SessionCommandPayload};
use zeron_proto::{
    ChatConfig, CreateWorktreeOutcome, EngineInfo, HarnessId, ProjectActionDraft, Space, ToolCall,
    WorkspaceScope,
};
use zeron_rpc::{LinkCache, RpcError, RpcReply, RpcService, methods, parse_params};

use crate::agent_accounts::AgentAccounts;
use crate::auth::Auth;
use crate::change_requests::CheckoutChangeRequests;
use crate::diff_sync::CheckoutDiffSync;
use crate::doc_host::DocHost;
use crate::project_actions::ProjectActionsStore;
use crate::registry::HarnessRegistry;
use crate::repos::{Repos, session_home_dir};
use crate::sessions::SessionsEngine;
use crate::terminals::Terminals;
use crate::uploads::Uploads;
use crate::workspace_host::WorkspaceHost;

const FILE_SEARCH_RPC_TIMEOUT: Duration = Duration::from_secs(6);
const FILE_SEARCH_FEATURED_PATHS: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatParams {
    chat_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListModelsParams {
    harness: HarnessId,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetHarnessEnabledParams {
    harness: HarnessId,
    enabled: bool,
}

async fn update_harness_enabled(
    registry: &HarnessRegistry,
    harness: HarnessId,
    enabled: bool,
) -> Result<(), RpcError> {
    registry
        .set_enabled(harness, enabled)
        .map_err(RpcError::Failed)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueCommandParams {
    chat_id: String,
    command: SessionCommandPayload,
    /// Queued attachments (bytes already committed locally as `pending://`
    /// refs) the engine delivers to a remote host AFTER the command is
    /// durably queued — never as a gate in front of it.
    #[serde(default)]
    transfers: Vec<crate::uploads::AttachmentTransfer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelayCommandParams {
    chat_id: String,
    /// The full command entry, client-minted id included — the exactly-once
    /// key the host claims in its processed ledger before executing.
    entry: zeron_doc::SessionCommandEntry,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TakeProjectActionSetupParams {
    chat_id: String,
    command_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueMessageParams {
    chat_id: String,
    text: String,
    #[serde(default)]
    attachments: Vec<String>,
    /// Keep this row visible during the current turn even when the harness
    /// supports mid-turn steering.
    #[serde(default)]
    hold_for_turn_end: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueuedMessageParams {
    chat_id: String,
    id: String,
    /// Present for UpdateQueuedMessage only; empty text deletes the row.
    #[serde(default)]
    text: String,
    /// Present for MoveQueuedMessage only.
    #[serde(default)]
    to_index: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BeginQueuedMessageEditParams {
    chat_id: String,
    id: String,
    editor_device_id: String,
    editor_instance_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenewQueuedMessageEditParams {
    chat_id: String,
    id: String,
    lease_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FinishQueuedMessageEditAction {
    Commit,
    Cancel,
    Discard,
    ReleaseUnchanged,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinishQueuedMessageEditParams {
    chat_id: String,
    id: String,
    lease_id: String,
    action: FinishQueuedMessageEditAction,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    expected_text_hash: Option<String>,
    #[serde(default)]
    attachments: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoPathParams {
    /// `repoPath` per §3.5 (the §2.1 shorthand `repo` is accepted as an alias).
    #[serde(alias = "repo")]
    repo_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutChangeRequestParams {
    cwd: String,
    #[serde(default)]
    branch: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchRefParams {
    /// The checkout to switch — a session's cwd (main folder or worktree).
    repo_path: String,
    ref_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorktreeParams {
    #[serde(alias = "repo")]
    repo_path: String,
    branch: String,
    #[serde(default)]
    space_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteWorktreeParams {
    #[serde(alias = "repo")]
    repo_path: String,
    #[serde(alias = "path")]
    worktree_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscardWorkingTreeParams {
    chat_id: String,
    checkout_id: String,
    expected_checksum: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListProjectActionsParams {
    space_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertProjectActionParams {
    space_id: String,
    #[serde(default)]
    action_id: Option<String>,
    action: ProjectActionDraft,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteProjectActionParams {
    space_id: String,
    action_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunProjectActionParams {
    space_id: String,
    chat_id: String,
    action_id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListFoldersParams {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileSearchParams {
    query: String,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    space_id: Option<String>,
    /// Existing linked worktree selected for a new chat. The engine accepts it
    /// only after verifying it against the space repository's worktree list.
    #[serde(default)]
    path: Option<String>,
}

fn tool_file_path(call: &ToolCall) -> Option<&str> {
    match call {
        ToolCall::ReadFile { path }
        | ToolCall::WriteFile { path, .. }
        | ToolCall::EditFile { path, .. } => Some(path),
        ToolCall::ApplyPatch { path } | ToolCall::Search { path, .. } => path.as_deref(),
        ToolCall::Exec { .. }
        | ToolCall::Glob { .. }
        | ToolCall::WebFetch { .. }
        | ToolCall::WebSearch { .. }
        | ToolCall::Todo { .. }
        | ToolCall::Mcp { .. }
        | ToolCall::Unknown { .. } => None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenTerminalParams {
    chat_id: String,
    cols: u16,
    rows: u16,
    /// Explicit working directory (new-chat canvas: the selected project
    /// folder, or `~`). When omitted, the chat row's cwd is used, then the
    /// space named by a `space-canvas:{spaceId}` chat id.
    #[serde(default)]
    cwd: Option<String>,
}

/// Matches the UI canvas panel key (`AppState::panel_session_key`).
const CANVAS_TERMINAL_PREFIX: &str = "space-canvas:";

/// A cwd the user (or a project-less chat) meant as "host home", not a folder.
fn meaningful_cwd(cwd: Option<String>) -> Option<String> {
    cwd.filter(|cwd| {
        let trimmed = cwd.trim();
        !trimmed.is_empty() && trimmed != "~"
    })
}

/// Space id encoded in a new-chat canvas terminal key, if any.
fn canvas_space_id(chat_id: &str) -> Option<&str> {
    chat_id
        .strip_prefix(CANVAS_TERMINAL_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Resolve the PTY cwd: a real explicit path wins, then the chat row, then
/// the project folder named by `space-canvas:{spaceId}`, then `~`.
/// The portable `~` marker is a fallback, not an override — otherwise a
/// canvas OpenTerminal that still says `~` (spaces watch not landed in the
/// UI) would ignore the selected project encoded in `chatId`.
fn resolve_open_terminal_cwd(
    explicit: Option<String>,
    chat_cwd: Option<String>,
    space_cwd: Option<String>,
) -> Result<String, &'static str> {
    let raw = meaningful_cwd(explicit)
        .or_else(|| meaningful_cwd(chat_cwd))
        .or_else(|| meaningful_cwd(space_cwd))
        .unwrap_or_else(|| "~".to_string());
    crate::repos::expand_home(&raw)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalIdParams {
    terminal_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeTerminalParams {
    terminal_id: String,
    #[serde(default)]
    after_seq: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteTerminalParams {
    terminal_id: String,
    /// Base64 input bytes (plain UTF-8 accepted leniently).
    data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizeTerminalParams {
    terminal_id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAgentAccountsParams {
    #[serde(default)]
    force_usage: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentAccountParams {
    harness: HarnessId,
    account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartAgentLoginParams {
    harness: HarnessId,
    /// Stamped by the requesting engine when it forwards the start: the
    /// device whose browser finishes the sign-in. The login's callback port
    /// is served over P2P to that device alone.
    #[serde(default)]
    requester_device_id: Option<String>,
    /// For agents that keep a login per model provider (OpenCode, Pi,
    /// Hermes): which provider to sign in to; `None` = the agent's default.
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginIdParams {
    login_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteAgentLoginParams {
    login_id: String,
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadChunkParams {
    upload_id: String,
    /// Base64 payload chunk.
    data: String,
    #[serde(default)]
    seq: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadCommitParams {
    upload_id: String,
    file_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadAttachmentChunkParams {
    path: String,
    #[serde(default)]
    offset: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchToolBlobParams {
    /// Doc-resident sidecar ref (`{chatId}/{partId}` or `…​.diff`).
    blob_ref: String,
}

/// The Mutate surface (feature-inventory §2 DataRpc), tagged by `op`.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum MutateParams {
    #[serde(rename_all = "camelCase")]
    CreateChat {
        chat_id: String,
        /// The project the chat is created in — fixes host device + base cwd.
        /// `None` mints a project-less chat: `deviceId` picks the host and the
        /// cwd defaults to `~` (expanded on the host at run time).
        #[serde(default)]
        space_id: Option<String>,
        /// Host device for a project-less chat; ignored when `spaceId` is set.
        #[serde(default)]
        device_id: Option<String>,
        #[serde(default)]
        config: Option<ChatConfig>,
        /// The picked ref, named on the row from the first frame (the footer
        /// read "Select ref" until the diff reconciler stamped it).
        #[serde(default)]
        branch: Option<String>,
        /// Cwd override (isolated-worktree path); default = the space's folder.
        #[serde(default)]
        cwd: Option<String>,
        /// The chat whose agent is creating this one (Zeron MCP); recorded
        /// on the row as `parentChatId` for orchestration trees.
        #[serde(default)]
        parent_chat_id: Option<String>,
    },
    /// Create a space (device + folder pair). Idempotent by id; a live
    /// duplicate `(deviceId, path)` no-ops. `gitDetected` is seeded from the
    /// picker's FolderEntry — the owning device's SpacesSync re-verifies.
    #[serde(rename_all = "camelCase")]
    CreateSpace {
        space_id: String,
        device_id: String,
        path: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        git_detected: bool,
    },
    /// LWW display-name set; `name: None` clears back to basename(path).
    #[serde(rename_all = "camelCase")]
    RenameSpace {
        space_id: String,
        #[serde(default)]
        name: Option<String>,
    },
    /// Hard delete: cascades to every chat (and session row) in the space.
    /// Live runs hosted here are interrupted best-effort.
    #[serde(rename_all = "camelCase")]
    DeleteSpace { space_id: String },
    #[serde(rename_all = "camelCase")]
    RenameChat { chat_id: String, title: String },
    /// Set the chat's checkout branch label — the sidebar's
    /// "project · branch" sub-line.
    #[serde(rename_all = "camelCase")]
    SetChatBranch { chat_id: String, branch: String },
    /// Retarget a chat onto another folder — mid-session switch to an
    /// EXISTING worktree (the picked ref's checkout). Next run starts a
    /// fresh harness conversation there (resume is cwd-scoped).
    #[serde(rename_all = "camelCase")]
    SetChatCwd { chat_id: String, cwd: String },
    /// Backdate a chat's activity timestamps (epoch ms) — the sidebar's
    /// relative-time column. Used by tooling/seeds; the doc fold sets these on
    /// real message traffic.
    #[serde(rename_all = "camelCase")]
    SetChatActivity {
        chat_id: String,
        #[serde(default)]
        last_message_at: Option<i64>,
        #[serde(default)]
        created_at: Option<i64>,
    },
    /// Re-home a chat to another device (tooling/seeds; device migration later).
    #[serde(rename_all = "camelCase")]
    SetChatHost { chat_id: String, device_id: String },
    #[serde(rename_all = "camelCase")]
    SetChatArchived { chat_id: String, archived: bool },
    /// Change one pin without replacing another device's edits.
    #[serde(rename_all = "camelCase")]
    ChangeSidebarPin {
        change: zeron_proto::SidebarPinChange,
    },
    /// Full-config replace on the chat row (zeron `SetChatConfig`): the
    /// composer's mid-session model / reasoning / options changes, LWW-synced
    /// so they survive restarts and reach every device.
    #[serde(rename_all = "camelCase")]
    SetChatConfig { chat_id: String, config: ChatConfig },
    /// Tombstone: removes the chats-map row; the session doc remains.
    #[serde(rename_all = "camelCase")]
    DeleteChat { chat_id: String },
    #[serde(rename_all = "camelCase")]
    RenameDevice { device_id: String, name: String },
    /// Synced seen marker (LWW + monotonic guard): clears the "completed"
    /// badge on every device. `at` is epoch ms; default = now.
    #[serde(rename_all = "camelCase")]
    MarkChatSeen {
        chat_id: String,
        #[serde(default)]
        at: Option<i64>,
    },
}

pub struct EngineRpc {
    sessions: SessionsEngine,
    doc_host: DocHost,
    workspace: WorkspaceHost,
    registry: std::sync::Arc<HarnessRegistry>,
    repos: Repos,
    workspace_files: crate::WorkspaceFiles,
    terminals: Terminals,
    project_actions: ProjectActionsStore,
    previews: Option<zeron_preview::PreviewService>,
    change_requests: CheckoutChangeRequests,
    diff_sync: CheckoutDiffSync,
    uploads: Uploads,
    agent_accounts: AgentAccounts,
    auth: Option<Auth>,
    links: Option<std::sync::Arc<LinkCache>>,
    updater: Option<zeron_update::Updater>,
    local_import: Option<crate::local_import::LocalImporter>,
    engine_info: EngineInfo,
}

impl EngineRpc {
    #[allow(clippy::too_many_arguments)] // engine assembly seam, not a public API
    pub fn new(
        sessions: SessionsEngine,
        doc_host: DocHost,
        workspace: WorkspaceHost,
        registry: std::sync::Arc<HarnessRegistry>,
        repos: Repos,
        workspace_files: crate::WorkspaceFiles,
        terminals: Terminals,
        project_actions: ProjectActionsStore,
        change_requests: CheckoutChangeRequests,
        diff_sync: CheckoutDiffSync,
        uploads: Uploads,
        agent_accounts: AgentAccounts,
        workspace_scope: WorkspaceScope,
    ) -> Self {
        let engine_info = EngineInfo {
            device_id: doc_host.device_id().to_string(),
            workspace_scope,
            cursor_sdk_version: Some(zeron_harness::CursorHarness::sdk_version().into()),
            capabilities: zeron_proto::capabilities::current(),
        };
        Self {
            sessions,
            doc_host,
            workspace,
            registry,
            repos,
            workspace_files,
            terminals,
            project_actions,
            previews: None,
            change_requests,
            diff_sync,
            uploads,
            agent_accounts,
            auth: None,
            links: None,
            updater: None,
            local_import: None,
            engine_info,
        }
    }

    pub fn with_previews(mut self, previews: zeron_preview::PreviewService) -> Self {
        self.previews = Some(previews);
        self
    }

    /// Attach the auth service (AuthStatus + AuthRpc mutations).
    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Attach the peer link cache — enables `targetDeviceId` relay forwarding.
    pub fn with_links(mut self, links: std::sync::Arc<LinkCache>) -> Self {
        self.links = Some(links);
        self
    }

    /// Attach the release checker (UpdateStatus stream + ApplyUpdate).
    pub fn with_updater(mut self, updater: zeron_update::Updater) -> Self {
        self.updater = Some(updater);
        self
    }

    /// Attach the local→synced profile importer (synced runtimes only).
    pub fn with_local_import(mut self, importer: crate::local_import::LocalImporter) -> Self {
        self.local_import = Some(importer);
        self
    }

    fn auth(&self) -> Result<&Auth, RpcError> {
        self.auth
            .as_ref()
            .ok_or_else(|| RpcError::Failed("auth unavailable".into()))
    }

    fn updater(&self) -> Result<&zeron_update::Updater, RpcError> {
        self.updater
            .as_ref()
            .ok_or_else(|| RpcError::Failed("updates unavailable".into()))
    }

    fn local_importer(&self) -> Result<&crate::local_import::LocalImporter, RpcError> {
        self.local_import
            .as_ref()
            .ok_or_else(|| RpcError::Failed("local import requires a synced workspace".into()))
    }

    fn local_project_action_space(&self, space_id: &str) -> Result<Space, RpcError> {
        let space = self
            .workspace
            .space(space_id)
            .map_err(|err| RpcError::Failed(err.to_string()))?
            .ok_or_else(|| RpcError::Failed("Project space not found".into()))?;
        if space.device_id != self.doc_host.device_id() {
            return Err(RpcError::Failed(
                "Project space belongs to another device".into(),
            ));
        }
        Ok(space)
    }

    /// Resolve a mention-search root from synced workspace rows. A client may
    /// name an existing linked worktree for a new chat, but it is verified
    /// against the space repository before any filesystem walk begins.
    async fn file_search_root(&self, p: &FileSearchParams) -> Result<std::path::PathBuf, RpcError> {
        let target = zeron_proto::WorkspaceTarget {
            chat_id: p.chat_id.clone(),
            space_id: p.space_id.clone(),
            checkout_path: p.path.clone(),
        };
        self.workspace_files
            .resolve_target(&target)
            .await
            .map(|workspace| workspace.root)
            .map_err(Into::into)
    }

    /// Catalogs also serve projectless sessions, which have no workspace space.
    /// Keep workspace validation for project targets and use only a known local
    /// chat's persisted cwd (or home) for a projectless conversation.
    async fn catalog_root(&self, p: &FileSearchParams) -> Result<std::path::PathBuf, RpcError> {
        if p.space_id.is_none() && p.path.is_none() {
            let Some(chat_id) = &p.chat_id else {
                return session_home_dir().map_err(|error| RpcError::Failed(error.to_string()));
            };
            let chat = self
                .workspace
                .chat(chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("chat not found".into()))?;
            if chat.device_id != self.doc_host.device_id() {
                return Err(RpcError::BadParams("chat belongs to another device".into()));
            }
            if chat.space_id.is_none() {
                let cwd = chat.cwd.as_deref().unwrap_or("~");
                return crate::repos::expand_home(cwd)
                    .map(std::path::PathBuf::from)
                    .map_err(|error| RpcError::Failed(error.to_string()));
            }
        }
        self.file_search_root(p).await
    }

    /// Accept only a checkout already named by a local chat or contained in a
    /// local space. Remote clients must not turn this RPC into an arbitrary path probe.
    async fn change_request_root(&self, cwd: &str) -> Result<std::path::PathBuf, RpcError> {
        let requested = std::path::PathBuf::from(cwd);
        let local_device = self.doc_host.device_id();
        let mut chats_rx = self.workspace.watch_chats();
        let mut spaces_rx = self.workspace.watch_spaces();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        loop {
            let chats = chats_rx.borrow_and_update().clone();
            if chats.iter().any(|chat| {
                chat.device_id == local_device
                    && chat.cwd.as_deref().map(std::path::Path::new) == Some(requested.as_path())
            }) {
                return Ok(requested);
            }

            let spaces = spaces_rx.borrow_and_update().clone();
            for space in spaces
                .iter()
                .filter(|space| space.device_id == local_device)
            {
                if let Some(checkout) = self
                    .repos
                    .workspace_checkout(std::path::Path::new(&space.path), &requested)
                    .await
                {
                    return Ok(checkout);
                }
            }

            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::select! {
                _ = chats_rx.changed() => {}
                _ = spaces_rx.changed() => {}
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        Err(RpcError::BadParams(
            "cwd is not a known checkout on this device".into(),
        ))
    }

    /// Most-recent-first paths the current chat actually touched, followed by
    /// files still changed in its checkout. The search worker validates and
    /// normalizes them against the resolved root before using them as ranking
    /// hints, so stale or out-of-workspace tool paths simply disappear.
    fn featured_file_paths(&self, chat_id: &str) -> Vec<String> {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(handle) = self.doc_host.open(chat_id)
            && let Ok(entries) = handle.doc().read_entries()
        {
            for entry in entries.into_iter().rev() {
                for part in entry.parts.into_iter().rev() {
                    if let MessagePart::Tool { call, .. } = part
                        && let Some(path) = tool_file_path(&call)
                        && !path.trim().is_empty()
                        && seen.insert(path.to_string())
                    {
                        paths.push(path.to_string());
                        if paths.len() == FILE_SEARCH_FEATURED_PATHS {
                            break;
                        }
                    }
                }
                if paths.len() == FILE_SEARCH_FEATURED_PATHS {
                    break;
                }
            }
        }

        if let Ok(Some(chat)) = self.workspace.chat(chat_id) {
            let diffs = self.diff_sync.watch_diffs().borrow().clone();
            let diff = chat
                .checkout_id
                .as_deref()
                .and_then(|id| diffs.iter().find(|diff| diff.checkout_id == id))
                .or_else(|| {
                    chat.cwd
                        .as_deref()
                        .and_then(|cwd| diffs.iter().find(|diff| diff.cwd == cwd))
                });
            if let Some(diff) = diff {
                for file in &diff.files {
                    if paths.len() == FILE_SEARCH_FEATURED_PATHS {
                        break;
                    }
                    if seen.insert(file.path.clone()) {
                        paths.push(file.path.clone());
                    }
                }
            }
        }
        paths
    }

    /// An agent login runs on `target`, but the browser that finishes it runs
    /// HERE: while the login waits on a loopback callback, this device's same
    /// port forwards to it over P2P ([`zeron_preview::login`]). The forwarder
    /// opens when a reply first names the port and closes when the login
    /// finishes, fails, is cancelled or its time runs out; a port taken here
    /// fails the login with that reason instead of stranding the browser.
    async fn forward_agent_login(
        &self,
        target: &str,
        method: &str,
        mut params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        use zeron_proto::{AgentLoginPoll, AgentLoginStart, AgentLoginStatus};
        let login_id = params
            .get("loginId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if method == methods::START_AGENT_LOGIN
            && let Some(object) = params.as_object_mut()
        {
            object.insert(
                "requesterDeviceId".into(),
                serde_json::json!(self.doc_host.device_id()),
            );
        }
        let reply = self.forward(target, method, params).await;
        let Some(previews) = &self.previews else {
            return reply;
        };
        let value = match &reply {
            Ok(RpcReply::Value(value)) => Some(value.clone()),
            _ => None,
        };
        match method {
            methods::START_AGENT_LOGIN => {
                let Some(start) =
                    value.and_then(|v| serde_json::from_value::<AgentLoginStart>(v).ok())
                else {
                    return reply;
                };
                if let Some(port) = start.callback_port
                    && let Err(error) = if crate::agent_accounts::tunnel_port_allowed(
                        port,
                        Some(start.url.as_str()),
                    ) {
                        previews
                            .open_login_tunnel(&start.login_id, target, port, LOGIN_TUNNEL_TTL)
                            .await
                    } else {
                        Err(anyhow::anyhow!(
                            "The other device reported an unexpected sign-in port."
                        ))
                    }
                {
                    self.cancel_remote_login(target, &start.login_id).await;
                    return Err(RpcError::Failed(error.to_string()));
                }
                reply
            }
            methods::POLL_AGENT_LOGIN => {
                let Some(login_id) = login_id else {
                    return reply;
                };
                let poll = value.and_then(|v| serde_json::from_value::<AgentLoginPoll>(v).ok());
                match poll {
                    Some(poll) if poll.status == AgentLoginStatus::Pending => {
                        if let Some(port) = poll.callback_port
                            && let Err(error) = if crate::agent_accounts::tunnel_port_allowed(
                                port,
                                poll.url.as_deref(),
                            ) {
                                previews
                                    .open_login_tunnel(&login_id, target, port, LOGIN_TUNNEL_TTL)
                                    .await
                            } else {
                                Err(anyhow::anyhow!(
                                    "The other device reported an unexpected sign-in port."
                                ))
                            }
                        {
                            self.cancel_remote_login(target, &login_id).await;
                            return RpcReply::value(&AgentLoginPoll {
                                status: AgentLoginStatus::Error,
                                message: Some(error.to_string()),
                                url: None,
                                callback_port: None,
                            });
                        }
                        reply
                    }
                    // Done, failed, expired, or unreachable: the login is over.
                    _ => {
                        previews.close_login_tunnel(&login_id);
                        reply
                    }
                }
            }
            _ => {
                if let Some(login_id) = login_id {
                    previews.close_login_tunnel(&login_id);
                }
                reply
            }
        }
    }

    async fn cancel_remote_login(&self, target: &str, login_id: &str) {
        let params = serde_json::json!({ "loginId": login_id, "targetDeviceId": target });
        if let Err(error) = self
            .forward(target, methods::CANCEL_AGENT_LOGIN, params)
            .await
        {
            tracing::debug!(%error, "cancelling the remote login failed (best-effort)");
        }
    }

    /// Forward a device-addressed call over the target device's relay. On transport
    /// failure the cached link is invalidated so the next call re-dials.
    async fn forward(
        &self,
        target: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let Some(links) = &self.links else {
            return Err(RpcError::Failed(format!(
                "cannot reach device {target}: remote routing unavailable (offline)"
            )));
        };
        let client = links.client(target).await?;
        if is_stream_method(method) {
            // Streams are unbounded by design (a quiet WATCH_* is healthy);
            // only unary calls below get the reply deadline.
            if matches!(
                method,
                methods::WATCH_CHECKOUT_CHANGE_REQUEST | methods::WATCH_WORKSPACE_GIT_STATUS
            ) {
                let rx = match client.subscribe_checked(method, params).await {
                    Ok(rx) => rx,
                    Err(err) => {
                        if should_invalidate_link(&err) {
                            links.invalidate(target);
                        }
                        return Err(err);
                    }
                };
                let stream = futures::stream::unfold((rx, client), |(mut rx, client)| async move {
                    rx.recv().await.map(|item| (item, (rx, client)))
                });
                return Ok(RpcReply::Stream(stream.boxed()));
            }
            let rx = match client.subscribe_scoped(method, params).await {
                Ok(rx) => rx,
                Err(err) => {
                    if should_invalidate_link(&err) {
                        links.invalidate(target);
                    }
                    return Err(err);
                }
            };
            // Pipe remote items; the held client keeps the link's RpcClient alive for
            // the stream's lifetime. A remote error just ends the stream (the relay
            // link-down path fails pending calls; stream receivers close).
            let stream = futures::stream::unfold((rx, client), |(mut rx, client)| async move {
                rx.recv().await.map(|item| (item, (rx, client)))
            });
            return Ok(RpcReply::Stream(stream.boxed()));
        }
        let deadline = forward_deadline(method);
        match tokio::time::timeout(deadline, client.call(method, params)).await {
            Ok(Ok(value)) => Ok(RpcReply::Value(value)),
            Ok(Err(err)) => {
                if should_invalidate_link(&err) {
                    links.invalidate(target);
                }
                Err(err)
            }
            Err(_) => {
                // No reply inside the deadline. The link may be a zombie — the
                // relay's auto-pong keeps a dead host socket looking alive
                // (ws3 auto-pong incident) — so drop it; the next call re-dials.
                // NOTE: the remote may still complete the forwarded work; the
                // caller sees a retryable failure instead of hanging forever
                // (the "Sending…" wedge, 2026-08-18).
                links.invalidate(target);
                Err(RpcError::Transport(format!(
                    "no reply from device {target} for {method} within {}s",
                    deadline.as_secs()
                )))
            }
        }
    }

    fn mutate(&self, params: MutateParams) -> Result<(), RpcError> {
        let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
        match params {
            MutateParams::CreateChat {
                chat_id,
                space_id,
                device_id,
                config,
                branch,
                cwd,
                parent_chat_id,
            } => {
                self.workspace
                    .create_chat_with_parent(
                        &chat_id,
                        space_id.as_deref(),
                        device_id.as_deref(),
                        config,
                        cwd,
                        parent_chat_id,
                    )
                    .map_err(failed)?;
                if let Some(branch) = branch.as_deref().filter(|b| !b.is_empty()) {
                    self.workspace
                        .set_chat_branch(&chat_id, branch)
                        .map_err(failed)?;
                }
                Ok(())
            }
            MutateParams::CreateSpace {
                space_id,
                device_id,
                path,
                name,
                git_detected,
            } => self
                .workspace
                .create_space(&space_id, &device_id, &path, name, git_detected)
                .map_err(failed),
            MutateParams::RenameSpace { space_id, name } => self
                .workspace
                .rename_space(&space_id, name.as_deref())
                .map_err(failed)
                .map(drop),
            MutateParams::DeleteSpace { space_id } => {
                let deleted = self.workspace.delete_space(&space_id).map_err(failed)?;
                // Best-effort teardown of live runs we host for the deleted chats
                // (the doc rows are already tombstoned; a straggler run would only
                // write into an orphaned session doc).
                let sessions = self.sessions.clone();
                let doc_host = self.doc_host.clone();
                let chat_ids = deleted.chat_ids;
                tokio::spawn(async move {
                    for chat_id in chat_ids {
                        if let Err(err) = sessions.interrupt(&chat_id).await {
                            tracing::debug!(chat = %chat_id, error = %err, "deleteSpace interrupt skipped");
                        }
                        doc_host.purge_chat(&chat_id);
                    }
                });
                Ok(())
            }
            MutateParams::RenameChat { chat_id, title } => self
                .workspace
                .rename_chat(&chat_id, &title)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatBranch { chat_id, branch } => self
                .workspace
                .set_chat_branch(&chat_id, &branch)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatCwd { chat_id, cwd } => self
                .workspace
                .set_chat_cwd(&chat_id, &cwd)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatActivity {
                chat_id,
                last_message_at,
                created_at,
            } => self
                .workspace
                .set_chat_activity(&chat_id, last_message_at, created_at)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatHost { chat_id, device_id } => self
                .workspace
                .set_chat_host(&chat_id, &device_id)
                .map_err(failed)
                .map(drop),
            MutateParams::SetChatArchived { chat_id, archived } => self
                .workspace
                .set_chat_archived(&chat_id, archived)
                .map_err(failed)
                .map(drop),
            MutateParams::ChangeSidebarPin { change } => {
                self.workspace.change_sidebar_pin(&change).map_err(failed)
            }
            MutateParams::SetChatConfig { chat_id, config } => self
                .workspace
                .set_chat_config(&chat_id, &config)
                .map_err(failed)
                .map(drop),
            MutateParams::DeleteChat { chat_id } => {
                self.workspace.delete_chat(&chat_id).map_err(failed)?;
                self.doc_host.purge_chat(&chat_id);
                Ok(())
            }
            MutateParams::RenameDevice { device_id, name } => self
                .workspace
                .rename_device(&device_id, &name)
                .map_err(failed)
                .map(drop),
            MutateParams::MarkChatSeen { chat_id, at } => {
                let at = at
                    .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                    .unwrap_or_else(chrono::Utc::now);
                self.workspace
                    .mark_chat_seen(&chat_id, at)
                    .map_err(failed)
                    .map(drop)
            }
        }
    }
}

/// An RPC rejection is scoped to the requested capability. Only a broken
/// transport means the shared device link itself cannot carry other calls.
fn should_invalidate_link(error: &RpcError) -> bool {
    matches!(error, RpcError::Closed | RpcError::Transport(_))
}

/// Reply deadline for a relay-forwarded unary call. The relay is WebSocket
/// frames through a DO: a dropped frame (host socket replaced mid-call, DO
/// restart) loses the reply SILENTLY — the DO's auto-pong keeps the client
/// socket looking healthy — and an unbounded await wedged callers forever
/// (the composer's permanent "Sending…", 2026-08-18). Network-bound git and
/// update methods get a long leash; worktree creation checks out a full tree;
/// everything else is interactive and must fail fast.
#[derive(Default)]
pub(crate) struct Installations(
    std::sync::Mutex<std::collections::HashMap<HarnessId, zeron_harness::CancellationToken>>,
);

struct Installing<'a> {
    installs: &'a Installations,
    harness: HarnessId,
    cancel: zeron_harness::CancellationToken,
}
impl Drop for Installing<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.installs
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.harness);
    }
}
impl Installations {
    fn begin(&self, harness: HarnessId) -> Result<Installing<'_>, RpcError> {
        let mut installs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if installs.contains_key(&harness) {
            return Err(RpcError::Failed("already installing".into()));
        }
        let cancel = zeron_harness::CancellationToken::new();
        installs.insert(harness, cancel.clone());
        Ok(Installing {
            installs: self,
            harness,
            cancel,
        })
    }
    fn cancel(&self, harness: HarnessId) {
        if let Some(cancel) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&harness)
        {
            cancel.cancel();
        }
    }
}

async fn run_requested_install(
    harness: HarnessId,
    cancel: zeron_harness::CancellationToken,
) -> Result<(), zeron_harness::HarnessError> {
    #[cfg(test)]
    if let Ok(script) = std::env::var(format!("ZERON_INSTALLER_COMMAND_{harness:?}").to_uppercase())
    {
        return zeron_harness::install::install_with_command(harness, &script, cancel).await;
    }
    zeron_harness::install::install_harness(harness, cancel).await
}

async fn install_harness_with<F, Fut>(
    registry: &HarnessRegistry,
    harness: HarnessId,
    install: F,
) -> Result<Vec<crate::registry::HarnessDescriptor>, RpcError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), zeron_harness::HarnessError>>,
{
    if !zeron_harness::install::can_install(harness) {
        return Err(RpcError::Failed(
            "No supported installer or required tools available on this device".into(),
        ));
    }
    install()
        .await
        .map_err(|error| RpcError::Failed(error.to_string()))?;
    Ok(registry.descriptors())
}

fn forward_deadline(method: &str) -> std::time::Duration {
    use std::time::Duration;
    match method {
        methods::CLONE_REPO | methods::FETCH_ALL | methods::APPLY_UPDATE => {
            Duration::from_secs(15 * 60)
        }
        methods::INSTALL_HARNESS => Duration::from_secs(15 * 60),
        methods::CREATE_WORKTREE => Duration::from_secs(120),
        // Allow the adapter discovery budget plus relay and shutdown overhead.
        methods::LIST_MODELS | methods::LIST_COMMANDS => Duration::from_secs(100),
        _ => Duration::from_secs(30),
    }
}

/// How long this device forwards a remote login's callback at most — the
/// running engine reaps an abandoned login after the same 15 minutes.
const LOGIN_TUNNEL_TTL: Duration = Duration::from_secs(15 * 60);

/// ControlRpc methods that honor `targetDeviceId` (feature-inventory §2.1). Extend this
/// list (plus [`is_stream_method`] for streams) to make more of the surface
/// device-addressable — the handlers themselves need no changes.
fn forwardable(method: &str) -> bool {
    matches!(
        method,
        methods::FORK_SIDE_CHAT
            | methods::LIST_HARNESSES
            | methods::INSTALL_HARNESS
            | methods::CANCEL_INSTALL
            | methods::GET_TITLE_SETTINGS
            | methods::SET_TITLE_SETTINGS
            | methods::SET_HARNESS_ENABLED
            | methods::LIST_MODELS
            | methods::LIST_SKILLS
            | methods::LIST_COMMANDS
            | methods::QUEUE_COMMAND
            | methods::TAKE_PROJECT_ACTION_SETUP
            | methods::WATCH_DOC_MESSAGES
            // The queue lives on the chat doc, and only its host may send from
            // it — same addressing as the command ledger next door.
            | methods::WATCH_QUEUE
            | methods::QUEUE_MESSAGE
            | methods::UPDATE_QUEUED_MESSAGE
            | methods::BEGIN_QUEUED_MESSAGE_EDIT
            | methods::RENEW_QUEUED_MESSAGE_EDIT
            | methods::FINISH_QUEUED_MESSAGE_EDIT
            | methods::MOVE_QUEUED_MESSAGE
            | methods::REMOVE_QUEUED_MESSAGE
            | methods::SEND_QUEUED_MESSAGE_NOW
            | methods::STEER_QUEUED_MESSAGE_NOW
            // Repos/worktrees/folders are device-local filesystem state.
            | methods::LIST_REPOS
            | methods::ADD_REPO
            | methods::CLONE_REPO
            | methods::CREATE_REPO
            | methods::LIST_BRANCHES
            | methods::LIST_REFS
            | methods::LIST_GIT_HISTORY
            | methods::SEARCH_GIT_HISTORY
            | methods::RESOLVE_GIT_AVATARS
            | methods::FETCH_ALL
            | methods::SWITCH_REF
            | methods::LIST_FOLDERS
            | methods::LIST_DRIVES
            | methods::SEARCH_FILES
            | methods::LIST_WORKSPACE_DIRECTORY
            | methods::SEARCH_WORKSPACE_FILES
            | methods::READ_WORKSPACE_IMAGE
            | methods::READ_WORKSPACE_FILE
            | methods::WRITE_WORKSPACE_FILE
            | methods::WATCH_WORKSPACE_FILES
            | methods::CREATE_WORKTREE
            | methods::DELETE_WORKTREE
            // Project Actions live in the owning engine's private profile store.
            | methods::LIST_PROJECT_ACTIONS
            | methods::UPSERT_PROJECT_ACTION
            | methods::DELETE_PROJECT_ACTION
            | methods::RUN_PROJECT_ACTION
            // Checkout diffs are produced on the device holding the checkout.
            | methods::WATCH_CHECKOUT_DIFFS
            | methods::WATCH_WORKSPACE_GIT_STATUS
            | methods::WATCH_CHECKOUT_CHANGE_REQUEST
            | methods::GET_CHECKOUT_DIFF
            | methods::DISCARD_WORKING_TREE
            | methods::GET_CHECKOUT_FILE_DIFF_TEXT
            // Terminals live on the chat's host device.
            | methods::OPEN_TERMINAL
            | methods::SUBSCRIBE_TERMINAL
            | methods::WRITE_TERMINAL
            | methods::RESIZE_TERMINAL
            | methods::CLOSE_TERMINAL
            // Agent accounts are per-device CLI logins (the device switcher
            // retargets which device's logins are shown).
            | methods::LIST_AGENT_ACCOUNTS
            | methods::ACTIVATE_AGENT_ACCOUNT
            | methods::FORGET_AGENT_ACCOUNT
            | methods::START_AGENT_LOGIN
            | methods::COMPLETE_AGENT_LOGIN
            | methods::POLL_AGENT_LOGIN
            | methods::CANCEL_AGENT_LOGIN
            // Uploads/attachments target the chat's host device (the agent reads
            // the committed file from that device's disk).
            | methods::UPLOAD_CHUNK
            | methods::UPLOAD_COMMIT
            | methods::READ_ATTACHMENT_CHUNK
            // Updates report/apply on the device whose binary they concern.
            | methods::UPDATE_STATUS
            | methods::APPLY_UPDATE
    )
}

/// Forwardable methods whose reply is a stream (proxied item-by-item).
fn is_stream_method(method: &str) -> bool {
    matches!(
        method,
        methods::WATCH_DOC_MESSAGES
            | methods::WATCH_QUEUE
            | methods::SUBSCRIBE_TERMINAL
            | methods::WATCH_CHECKOUT_DIFFS
            | methods::WATCH_WORKSPACE_GIT_STATUS
            | methods::WATCH_CHECKOUT_CHANGE_REQUEST
            | methods::WATCH_WORKSPACE_FILES
            | methods::UPDATE_STATUS
    )
}

/// A watch receiver as a stream: current value first, then every change.
fn watch_stream<T>(rx: watch::Receiver<T>) -> BoxStream<'static, serde_json::Value>
where
    T: serde::Serialize + Clone + Send + Sync + 'static,
{
    futures::stream::unfold((rx, false), |(mut rx, emitted)| async move {
        if emitted {
            rx.changed().await.ok()?;
        }
        let value = {
            let borrowed = rx.borrow_and_update();
            serde_json::to_value(&*borrowed).ok()?
        };
        Some((value, (rx, true)))
    })
    .boxed()
}

/// The transcript watch as delta frames (`zeron_doc::transcript_delta`): a
/// full `reset` first, then only changed entries per commit — the whole-Vec
/// serialization here was the per-tick cost that scaled with transcript size.
fn doc_messages_stream(
    rx: watch::Receiver<crate::doc_host::TranscriptSnapshot>,
    doc: std::sync::Arc<zeron_doc::SessionDoc>,
) -> BoxStream<'static, serde_json::Value> {
    use zeron_doc::transcript_delta::{TranscriptFrame, diff_transcript};
    futures::stream::unfold(
        (
            rx,
            None::<crate::doc_host::TranscriptSnapshot>,
            doc,
            None,
            zeron_doc::TranscriptBaseline::default(),
        ),
        |(mut rx, mut prev, doc, mut previous_usage, mut opening_baseline)| async move {
            loop {
                if prev.is_some() {
                    rx.changed().await.ok()?;
                }
                // Watchers retain the immutable published snapshot. Each
                // connection used to deep-copy the entire transcript here.
                let current = rx.borrow_and_update().clone();
                let frame = match prev.as_ref() {
                    None => TranscriptFrame::reset(&current.entries),
                    Some(prev) => diff_transcript(&prev.entries, &current.entries),
                };
                let replay_baseline = match prev.as_ref() {
                    None => {
                        opening_baseline = zeron_doc::TranscriptBaseline::capture(&current.entries);
                        Some(opening_baseline.clone())
                    }
                    Some(prev)
                        if !std::sync::Arc::ptr_eq(
                            &prev.replay_baseline,
                            &current.replay_baseline,
                        ) =>
                    {
                        // The tracker only observes changes after attach; its
                        // baseline omits unchanged cached parts. Preserve this
                        // subscription's opening cutoff without capturing live
                        // appends or sharing another viewer's later cutoff.
                        // Ordinary live updates never rebuild this metadata.
                        let mut baseline = (*current.replay_baseline).clone();
                        for (entry, parts) in &opening_baseline.entries {
                            let merged = baseline.entries.entry(entry.clone()).or_default();
                            for (part, &len) in parts {
                                let cutoff = merged.entry(part.clone()).or_default();
                                *cutoff = (*cutoff).max(len);
                            }
                        }
                        Some(baseline)
                    }
                    _ => None,
                };
                prev = Some(current);
                // No-op commits (a second watcher attaching, command-only
                // changes) produce empty deltas — skip the frame entirely.
                let usage = doc.context_usage();
                if frame.is_empty_delta() && usage == previous_usage && replay_baseline.is_none() {
                    continue;
                }
                previous_usage = usage;
                let value = serde_json::to_value(zeron_doc::TranscriptUpdate {
                    frame,
                    context_usage: usage,
                    replay_baseline,
                })
                .ok()?;
                return Some((value, (rx, prev, doc, previous_usage, opening_baseline)));
            }
        },
    )
    .boxed()
}

/// First paint reads only the local tail; full history is deferred until the
/// next stream poll. No network dependency or persisted truncation.
async fn opening_doc_messages_stream(
    host: crate::doc_host::DocHost,
    chat_id: String,
) -> Result<BoxStream<'static, serde_json::Value>, RpcError> {
    let (handle, preview) = tokio::task::spawn_blocking(move || {
        let handle = host.open(&chat_id)?;
        let entries = handle.doc().read_opening_tail(128)?;
        let mut preview = serde_json::to_value(zeron_doc::TranscriptUpdate {
            frame: zeron_doc::TranscriptFrame::reset(&entries),
            context_usage: handle.doc().context_usage(),
            replay_baseline: Some(zeron_doc::TranscriptBaseline::capture(&entries)),
        })
        .map_err(|e| crate::EngineError::Other(e.to_string()))?;
        preview["historyPending"] = serde_json::Value::Bool(true);
        Ok::<_, crate::EngineError>((handle, preview))
    })
    .await
    .map_err(|e| RpcError::Failed(e.to_string()))?
    .map_err(|e| RpcError::Failed(e.to_string()))?;
    // Do not build the full mirror before yielding the preview.
    // The next poll attaches normally and begins with a complete
    // authoritative reset; subsequent frames use normal deltas.
    let full = futures::stream::once(async move {
        match tokio::task::spawn_blocking(move || (handle.watch_messages(), handle.doc_arc())).await
        {
            Ok((rx, doc)) => doc_messages_stream(rx, doc),
            Err(error) => {
                tracing::warn!(%error, "transcript opening failed");
                futures::stream::empty().boxed()
            }
        }
    })
    .flatten();
    Ok(futures::stream::once(async move { preview })
        .chain(full)
        .boxed())
}

/// Authentication-only RPC surface used while the headed app is waiting for a
/// production WorkOS session. Keeping this independent from [`EngineRpc`] lets
/// the UI show its sign-in and organization gates before identity-scoped Loro
/// stores are opened.
#[derive(Clone)]
pub struct AuthRpc {
    auth: Auth,
}

impl AuthRpc {
    pub fn new(auth: Auth) -> Self {
        Self { auth }
    }

    pub fn handles(method: &str) -> bool {
        matches!(
            method,
            methods::AUTH_STATUS
                | methods::SIGN_IN
                | methods::SIGN_IN_HEADLESS
                | methods::COMPLETE_SIGN_IN
                | methods::SIGN_OUT
                | methods::LIST_ORGS
                | methods::CREATE_ORG
                | methods::SELECT_ORG
        )
    }
}

#[async_trait]
impl RpcService for AuthRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::AUTH_STATUS => Ok(RpcReply::Stream(watch_stream(self.auth.watch_state()))),
            methods::SIGN_IN => {
                let url = self
                    .auth
                    .start_sign_in()
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "url": url }))
            }
            methods::SIGN_IN_HEADLESS => {
                let url = self.auth.start_headless_sign_in();
                RpcReply::value(&serde_json::json!({ "url": url }))
            }
            methods::COMPLETE_SIGN_IN => {
                #[derive(Deserialize)]
                struct P {
                    code: String,
                }
                let p: P = parse_params(params)?;
                self.auth
                    .complete_sign_in(&p.code)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::SIGN_OUT => {
                self.auth.sign_out();
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::LIST_ORGS => {
                let orgs = self
                    .auth
                    .list_orgs()
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "orgs": orgs }))
            }
            methods::CREATE_ORG => {
                #[derive(Deserialize)]
                struct P {
                    name: String,
                }
                let p: P = parse_params(params)?;
                self.auth
                    .create_org(&p.name)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::SELECT_ORG => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    organization_id: String,
                }
                let p: P = parse_params(params)?;
                self.auth
                    .select_org(&p.organization_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            _ => Err(RpcError::UnknownMethod(method.to_string())),
        }
    }
}

#[async_trait]
impl RpcService for EngineRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        // Device-addressed routing: forward calls that target another device over its
        // relay. The target compares the id to its own, so forwards cannot loop.
        if forwardable(method)
            && let Some(target) = params.get("targetDeviceId").and_then(|v| v.as_str())
            && target != self.doc_host.device_id()
        {
            let target = target.to_string();
            if matches!(
                method,
                methods::START_AGENT_LOGIN
                    | methods::POLL_AGENT_LOGIN
                    | methods::COMPLETE_AGENT_LOGIN
                    | methods::CANCEL_AGENT_LOGIN
            ) {
                return self.forward_agent_login(&target, method, params).await;
            }
            return self.forward(&target, method, params).await;
        }
        if AuthRpc::handles(method) {
            return AuthRpc::new(self.auth()?.clone())
                .handle(method, params)
                .await;
        }
        match method {
            methods::ENGINE_INFO => RpcReply::value(&self.engine_info),
            methods::ENGINE_READY => RpcReply::value(&serde_json::json!({ "ready": true })),
            methods::LIST_HARNESSES => RpcReply::value(&self.registry.descriptors()),
            methods::INSTALL_HARNESS => {
                let p: ListModelsParams = parse_params(params)?;
                let installing = self.registry.installs.begin(p.harness)?;
                let descriptors = install_harness_with(&self.registry, p.harness, || {
                    run_requested_install(p.harness, installing.cancel.clone())
                })
                .await?;
                RpcReply::value(&descriptors)
            }
            methods::CANCEL_INSTALL => {
                let p: ListModelsParams = parse_params(params)?;
                self.registry.installs.cancel(p.harness);
                RpcReply::value(&serde_json::json!({}))
            }
            methods::GET_TITLE_SETTINGS => RpcReply::value(&self.registry.title_settings()),
            methods::SET_TITLE_SETTINGS => {
                let p: crate::registry::TitleSettings = parse_params(params)?;
                self.registry
                    .set_title_settings(p)
                    .map_err(RpcError::Failed)?;
                RpcReply::value(&self.registry.title_settings())
            }
            methods::SET_HARNESS_ENABLED => {
                let p: SetHarnessEnabledParams = parse_params(params)?;
                update_harness_enabled(&self.registry, p.harness, p.enabled).await?;
                // Fresh catalog in the reply: the page repaints from it in one
                // round trip, and a refused/raced toggle self-corrects.
                RpcReply::value(&self.registry.descriptors())
            }
            methods::LIST_MODELS => {
                let p: ListModelsParams = parse_params(params)?;
                let harness = self
                    .registry
                    .resolve(p.harness)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let models = crate::model_catalogs::list(self.repos.data_dir(), harness, p.force)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&models)
            }
            methods::LIST_SKILLS => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    harness: HarnessId,
                    #[serde(default)]
                    chat_id: Option<String>,
                    #[serde(default)]
                    space_id: Option<String>,
                    #[serde(default)]
                    path: Option<String>,
                }
                let p: Params = parse_params(params)?;
                let root = self
                    .catalog_root(&FileSearchParams {
                        query: String::new(),
                        chat_id: p.chat_id,
                        space_id: p.space_id,
                        path: p.path,
                    })
                    .await?;
                let harness = self
                    .registry
                    .resolve(p.harness)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let skills = harness
                    .skills(&root)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&skills)
            }
            methods::LIST_COMMANDS => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    harness: HarnessId,
                    #[serde(default)]
                    chat_id: Option<String>,
                    #[serde(default)]
                    space_id: Option<String>,
                    #[serde(default)]
                    path: Option<String>,
                }
                let p: Params = parse_params(params)?;
                let root = self
                    .catalog_root(&FileSearchParams {
                        query: String::new(),
                        chat_id: p.chat_id,
                        space_id: p.space_id,
                        path: p.path,
                    })
                    .await?;
                let harness = self
                    .registry
                    .resolve(p.harness)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let commands = harness
                    .commands_for(&root)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&commands)
            }
            methods::QUEUE_COMMAND => {
                let p: QueueCommandParams = parse_params(params)?;
                let command_id = self
                    .doc_host
                    .queue_command_with_transfers(&p.chat_id, p.command, p.transfers)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "commandId": command_id }))
            }
            methods::TAKE_PROJECT_ACTION_SETUP => {
                let p: TakeProjectActionSetupParams = parse_params(params)?;
                let outcome = self
                    .project_actions
                    .take_setup_handoff(&p.command_id, &p.chat_id);
                match outcome {
                    Some(outcome) => RpcReply::value(&serde_json::json!({
                        "ready": true,
                        "setupAction": outcome.setup_action,
                        "setupError": outcome.setup_error,
                    })),
                    None => RpcReply::value(&serde_json::json!({ "ready": false })),
                }
            }
            methods::RETRY_DELIVERY => {
                let p: ChatParams = parse_params(params)?;
                self.doc_host
                    .retry_delivery(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({}))
            }
            methods::RELAY_COMMAND => {
                let p: RelayCommandParams = parse_params(params)?;
                let outcome = self
                    .doc_host
                    .ingest_relayed_command(&p.chat_id, p.entry)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "outcome": outcome }))
            }
            methods::FORK_SIDE_CHAT => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct ForkParams {
                    chat_id: String,
                    source_chat_id: String,
                    /// Where the fork hangs in the tree. Defaults to the
                    /// source; a side chat's own fork button passes the
                    /// side chat's parent so the copy lists as a sibling.
                    #[serde(default)]
                    parent_chat_id: Option<String>,
                }
                let p: ForkParams = parse_params(params)?;
                let parent_chat_id = p
                    .parent_chat_id
                    .clone()
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or_else(|| p.source_chat_id.clone());
                let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
                let source = self
                    .workspace
                    .chat(&p.source_chat_id)
                    .map_err(failed)?
                    .ok_or_else(|| RpcError::Failed("Source chat no longer exists".into()))?;
                if source.device_id != self.doc_host.device_id() {
                    return Err(RpcError::Failed(
                        "Fork must be created on the source device".into(),
                    ));
                }
                if let Some(existing) = self.workspace.chat(&p.chat_id).map_err(failed)? {
                    if existing.parent_chat_id.as_deref() == Some(parent_chat_id.as_str()) {
                        return RpcReply::value(&existing);
                    }
                    return Err(RpcError::Failed("Chat id already exists".into()));
                }
                let source_doc = self.doc_host.open(&p.source_chat_id).map_err(failed)?;
                let entries = source_doc
                    .doc()
                    .read_entries()
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let boundary = entries
                    .iter()
                    .rposition(|entry| {
                        entry.role == zeron_doc::MessageRole::Assistant
                            && entry.status == Some(zeron_doc::MessageStatus::Complete)
                    })
                    .ok_or_else(|| {
                        RpcError::Failed(
                            "Wait for a completed response before starting a side chat".into(),
                        )
                    })?;
                let mut chat = source.clone();
                chat.id = p.chat_id;
                chat.parent_chat_id = Some(parent_chat_id);
                chat.title = None; // First side-chat turn receives its own generated title.
                chat.archived = false;
                chat.created_at = chrono::Utc::now();
                chat.last_message_at = None;
                chat.last_message_preview = None;
                chat.last_seen_at = None;
                chat.harness_session_id = None;
                chat.harness_session_cwd = None;
                chat.room_gen = Some(2);
                // Missing rows open on chat2. Persist history before publishing
                // the registry row so a crash cannot leave a discoverable empty fork.
                let target = self.doc_host.open(&chat.id).map_err(failed)?;
                let existing = target
                    .doc()
                    .read_entries()
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                for entry in entries[..=boundary]
                    .iter()
                    .filter(|entry| !existing.iter().any(|e| e.id == entry.id))
                {
                    let mut entry = entry.clone();
                    // Historical approvals belong to the source runtime; they
                    // must never block or send answers from the new composer.
                    for part in &mut entry.parts {
                        if let zeron_doc::MessagePart::Input { resolved, .. } = part {
                            *resolved = true;
                        }
                    }
                    target
                        .doc()
                        .push_message(&entry)
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                }
                // The seam: everything above came from the source. Its own
                // entry (system role, complete) so the copied history and the
                // fork's first turn never share a row.
                let marker_id = format!("fork:{}", chat.id);
                if !existing.iter().any(|e| e.id == marker_id) {
                    let source_title = source
                        .title
                        .clone()
                        .or_else(|| source.last_message_preview.clone())
                        .unwrap_or_else(|| "New session".into());
                    target
                        .doc()
                        .push_message(&zeron_doc::SessionMessageEntry {
                            duration_ms: None,
                            id: marker_id.clone(),
                            role: zeron_doc::MessageRole::System,
                            parts: vec![zeron_doc::MessagePart::Fork {
                                id: marker_id,
                                source_chat_id: source.id.clone(),
                                source_title,
                            }],
                            created_at: chrono::Utc::now().timestamp_millis(),
                            device_id: self.doc_host.device_id().to_owned(),
                            status: Some(zeron_doc::MessageStatus::Complete),
                            continuation_of: None,
                        })
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                }
                self.doc_host.persist_fork(&target).map_err(failed)?;
                self.workspace.import_chat_row(&chat).map_err(failed)?;
                RpcReply::value(&chat)
            }
            methods::FOCUS_CHAT => {
                let p: ChatParams = parse_params(params)?;
                self.doc_host
                    .focus_chat(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({}))
            }
            methods::WATCH_DOC_MESSAGES => {
                // Opt-in: older viewports retain the full-reset contract.
                let opening_tail = params
                    .get("openingTail")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let p: ChatParams = parse_params(params)?;
                if opening_tail {
                    return Ok(RpcReply::Stream(
                        opening_doc_messages_stream(self.doc_host.clone(), p.chat_id).await?,
                    ));
                }
                let handle = self
                    .doc_host
                    .open(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                Ok(RpcReply::Stream(doc_messages_stream(
                    handle.watch_messages(),
                    handle.doc_arc(),
                )))
            }
            methods::WATCH_QUEUE => {
                let p: ChatParams = parse_params(params)?;
                let handle = self
                    .doc_host
                    .open(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let rx = handle.watch_queue();
                Ok(RpcReply::Stream(
                    futures::stream::unfold((rx, true), |(mut rx, first)| async move {
                        if !first {
                            rx.changed().await.ok()?;
                        }
                        let items = rx.borrow_and_update().clone();
                        let value = serde_json::json!({ "items": items });
                        Some((value, (rx, false)))
                    })
                    .boxed(),
                ))
            }
            methods::QUEUE_MESSAGE => {
                let p: QueueMessageParams = parse_params(params)?;
                let id = self
                    .doc_host
                    .queue_message_with_behavior(
                        &p.chat_id,
                        &p.text,
                        p.attachments,
                        p.hold_for_turn_end,
                    )
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "id": id }))
            }
            methods::UPDATE_QUEUED_MESSAGE => {
                let p: QueuedMessageParams = parse_params(params)?;
                let changed = self
                    .doc_host
                    .update_queued_message(&p.chat_id, &p.id, &p.text)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "changed": changed }))
            }
            methods::BEGIN_QUEUED_MESSAGE_EDIT => {
                let p: BeginQueuedMessageEditParams = parse_params(params)?;
                let outcome = self
                    .doc_host
                    .begin_queued_message_edit(
                        &p.chat_id,
                        &p.id,
                        &p.editor_device_id,
                        &p.editor_instance_id,
                    )
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(
                    &serde_json::to_value(outcome).map_err(|e| RpcError::Failed(e.to_string()))?,
                )
            }
            methods::RENEW_QUEUED_MESSAGE_EDIT => {
                let p: RenewQueuedMessageEditParams = parse_params(params)?;
                let outcome = self
                    .doc_host
                    .renew_queued_message_edit(&p.chat_id, &p.id, &p.lease_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(
                    &serde_json::to_value(outcome).map_err(|e| RpcError::Failed(e.to_string()))?,
                )
            }
            methods::FINISH_QUEUED_MESSAGE_EDIT => {
                let p: FinishQueuedMessageEditParams = parse_params(params)?;
                let action = match p.action {
                    FinishQueuedMessageEditAction::Commit => {
                        crate::doc_host::FinishQueueEditAction::Commit
                    }
                    FinishQueuedMessageEditAction::Cancel => {
                        crate::doc_host::FinishQueueEditAction::Cancel
                    }
                    FinishQueuedMessageEditAction::Discard => {
                        crate::doc_host::FinishQueueEditAction::Discard
                    }
                    FinishQueuedMessageEditAction::ReleaseUnchanged => {
                        crate::doc_host::FinishQueueEditAction::ReleaseUnchanged
                    }
                };
                let outcome = self
                    .doc_host
                    .finish_queued_message_edit_with_attachments(
                        &p.chat_id,
                        &p.id,
                        &p.lease_id,
                        action,
                        p.text.as_deref(),
                        p.expected_text_hash.as_deref(),
                        p.attachments.as_deref(),
                    )
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(
                    &serde_json::to_value(outcome).map_err(|e| RpcError::Failed(e.to_string()))?,
                )
            }
            methods::MOVE_QUEUED_MESSAGE => {
                let p: QueuedMessageParams = parse_params(params)?;
                let changed = self
                    .doc_host
                    .move_queued_message(&p.chat_id, &p.id, p.to_index)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "changed": changed }))
            }
            methods::REMOVE_QUEUED_MESSAGE => {
                let p: QueuedMessageParams = parse_params(params)?;
                let removed = self
                    .doc_host
                    .remove_queued_message(&p.chat_id, &p.id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "removed": removed }))
            }
            methods::SEND_QUEUED_MESSAGE_NOW => {
                let p: QueuedMessageParams = parse_params(params)?;
                let sent = self
                    .doc_host
                    .send_queued_now(&p.chat_id, &p.id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "sent": sent }))
            }
            methods::STEER_QUEUED_MESSAGE_NOW => {
                let p: QueuedMessageParams = parse_params(params)?;
                let sent = self
                    .doc_host
                    .steer_queued_now(&p.chat_id, &p.id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "sent": sent }))
            }
            methods::PROBE_SYNC => {
                // Focus probes remain cheap. An explicit Retry may also allow
                // one fresh, shared auth attempt before its cooldown expires.
                if params.get("retry").and_then(serde_json::Value::as_bool) == Some(true)
                    && let Some(auth) = &self.auth
                {
                    auth.retry_refresh();
                }
                self.workspace.probe();
                self.doc_host.probe_open_chats();
                self.doc_host.probe_edge_reachability();
                RpcReply::value(&serde_json::json!({}))
            }
            methods::SYNC_STATUS => {
                fn room_json(s: &zeron_sync::RoomStatsSnapshot) -> serde_json::Value {
                    serde_json::json!({
                        "connected": s.connected,
                        "synced": s.synced,
                        "lastPushedMs": s.last_pushed_ms,
                        "lastAckMs": s.last_ack_ms,
                        "rejoins": s.rejoins,
                        "probes": s.probes,
                        "fullResyncs": s.full_resyncs,
                        "disconnects": s.disconnects,
                        "rejected": s.rejected,
                    })
                }
                fn chat2_json(s: &zeron_sync::ChatStatsSnapshot) -> serde_json::Value {
                    serde_json::json!({
                        "connected": s.connected,
                        "cursor": s.cursor,
                        "headSeq": s.head_seq,
                        "seqFloor": s.seq_floor,
                        "checkpointSeq": s.checkpoint_seq,
                        "checkpointSize": s.checkpoint_size,
                        "rowCount": s.row_count,
                        "rowBytes": s.row_bytes,
                        "pendingPushes": s.pending_pushes,
                        "rejoins": s.rejoins,
                        "disconnects": s.disconnects,
                        "rejected": s.rejected,
                        "serverResets": s.server_resets,
                    })
                }
                let workspace = self.workspace.sync_status();
                let chats: Vec<serde_json::Value> = self
                    .doc_host
                    .sync_statuses()
                    .iter()
                    .map(|(chat_id, room)| {
                        serde_json::json!({
                            "chatId": chat_id,
                            "room": room.as_ref().map(chat2_json),
                            "state": self.doc_host.chat_sync_state(chat_id),
                        })
                    })
                    .collect();
                RpcReply::value(&serde_json::json!({
                    "deviceId": self.doc_host.device_id(),
                    "nowMs": crate::now_ms(),
                    "workspace": workspace.as_ref().map(room_json),
                    "chats": chats,
                    "resources": self.doc_host.sync_resources(),
                }))
            }
            methods::WATCH_CONNECTIVITY => Ok(RpcReply::Stream(watch_stream(
                self.doc_host.watch_connectivity(),
            ))),
            methods::WATCH_TRANSFERS => Ok(RpcReply::Stream(watch_stream(
                self.doc_host.watch_transfers(),
            ))),
            methods::WATCH_PREVIEWS => {
                let p: zeron_proto::WatchPreviewsParams = parse_params(params)?;
                if self
                    .workspace
                    .chat(&p.chat_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?
                    .is_none()
                {
                    return Err(RpcError::Failed("Project session not found".into()));
                }
                let previews = self
                    .previews
                    .as_ref()
                    .ok_or_else(|| RpcError::Failed("Preview discovery unavailable".into()))?;
                let catalog = previews.catalog().clone();
                let changes = catalog.subscribe();
                let chats = self.workspace.watch_chats();
                let workspace = self.workspace.clone();
                // This subscription stays on the viewing device. A remote chat
                // selects advertised services, but its URL uses our local proxy.
                let stream = futures::stream::unfold(
                    (changes, chats, true, workspace, catalog, p.chat_id),
                    |(mut changes, mut chats, first, workspace, catalog, chat_id)| async move {
                        if !first {
                            tokio::select! {
                                result = changes.changed() => { if result.is_err() { return None; } }
                                result = chats.changed() => { if result.is_err() { return None; } }
                            }
                        }
                        let mut snapshot = changes.borrow_and_update().clone();
                        chats.borrow_and_update();
                        let chat = workspace.chat(&chat_id).ok().flatten();
                        let device = chat
                            .as_ref()
                            .map(|c| c.device_id.clone())
                            .unwrap_or_default();
                        snapshot.remote = device != catalog.device_id();
                        let cwd = chat.and_then(|c| c.cwd);
                        let cwd = cwd.map(|cwd| {
                            if snapshot.remote {
                                std::path::PathBuf::from(cwd)
                            } else {
                                std::path::PathBuf::from(&cwd)
                                    .canonicalize()
                                    .unwrap_or_else(|_| cwd.into())
                            }
                        });
                        snapshot.project_name = cwd
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy().into_owned());
                        snapshot.services.retain(|service| {
                            service.device_id == device
                                && cwd.as_ref().is_some_and(|cwd| {
                                    cwd == std::path::Path::new(&service.project_cwd)
                                })
                        });
                        let value = serde_json::to_value(snapshot).ok()?;
                        Some((value, (changes, chats, false, workspace, catalog, chat_id)))
                    },
                );
                Ok(RpcReply::Stream(Box::pin(stream)))
            }
            methods::WATCH_CHATS => {
                Ok(RpcReply::Stream(watch_stream(self.workspace.watch_chats())))
            }
            methods::WATCH_SIDEBAR_PREFERENCES => Ok(RpcReply::Stream(watch_stream(
                self.workspace.watch_sidebar_preferences(),
            ))),
            methods::WATCH_DEVICES => Ok(RpcReply::Stream(watch_stream(
                self.workspace.watch_devices(),
            ))),
            methods::WATCH_SPACES => Ok(RpcReply::Stream(watch_stream(
                self.workspace.watch_spaces(),
            ))),
            methods::WATCH_SESSIONS => {
                // Local live statuses merged with remote devices' workspace rows.
                let merged = self
                    .workspace
                    .merged_sessions_watch(self.sessions.watch_sessions());
                Ok(RpcReply::Stream(watch_stream(merged)))
            }
            methods::LOCAL_DEVICE => {
                RpcReply::value(&serde_json::json!({ "deviceId": self.doc_host.device_id() }))
            }
            methods::LOCAL_IMPORT_STATUS => {
                let importer = self.local_importer()?.clone();
                let status = tokio::task::spawn_blocking(move || importer.status())
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&status)
            }
            methods::IMPORT_LOCAL_WORKSPACE => {
                let importer = self.local_importer()?.clone();
                // Progress rides an unbounded channel: the importer is
                // blocking (sqlite + fs) and must never wedge on a slow
                // viewer; items are tiny and bounded by the chat count.
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
                tokio::task::spawn_blocking(move || {
                    let emit = |event: crate::local_import::ImportEvent| {
                        if let Ok(item) = serde_json::to_value(&event) {
                            let _ = tx.send(item);
                        }
                    };
                    if let Err(err) = importer.run(emit) {
                        tracing::error!(error = %err, "local import failed");
                        let _ = tx.send(serde_json::json!({
                            "kind": "summary",
                            "importedChats": 0, "importedSpaces": 0,
                            "skippedChats": 0, "skippedSpaces": 0,
                            "journalsCopied": 0, "ledgerRowsMerged": 0,
                            "errors": [format!("{err}")],
                        }));
                    }
                    // tx drops here — the stream ends after the summary item.
                });
                Ok(RpcReply::Stream(Box::pin(futures::stream::poll_fn(
                    move |cx| rx.poll_recv(cx),
                ))))
            }
            methods::UPDATE_STATUS => Ok(RpcReply::Stream(watch_stream(self.updater()?.watch()))),
            methods::APPLY_UPDATE => {
                let version = self
                    .updater()?
                    .apply()
                    .await
                    .map_err(|e| RpcError::Failed(format!("{e:#}")))?;
                RpcReply::value(&serde_json::json!({ "ok": true, "version": version }))
            }
            methods::MUTATE => {
                let p: MutateParams = parse_params(params)?;
                let sidebar_pins = matches!(&p, MutateParams::ChangeSidebarPin { .. });
                self.mutate(p)?;
                if sidebar_pins {
                    return RpcReply::value(&serde_json::json!({
                        "ok": true, "sidebarPreferences": self.workspace.sidebar_preferences_snapshot(),
                    }));
                }
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::WATCH_CHECKOUT_DIFFS => {
                Ok(RpcReply::Stream(watch_stream(self.diff_sync.watch_diffs())))
            }
            methods::WATCH_WORKSPACE_GIT_STATUS => {
                let request: zeron_proto::WatchWorkspaceFilesRequest = parse_params(params)?;
                let workspace = self.workspace_files.resolve_target(&request.target).await?;
                let rx = self.diff_sync.watch_git_statuses();
                // Only this authorized checkout crosses the connection. None means
                // unavailable, including plain folders and initial/restarting engines.
                let stream = futures::stream::unfold(
                    (rx, workspace.checkout_id, None, false),
                    |(mut rx, checkout_id, mut previous, mut emitted)| async move {
                        loop {
                            if emitted {
                                rx.changed().await.ok()?;
                            }
                            let next = rx
                                .borrow_and_update()
                                .iter()
                                .find(|s| s.checkout_id == checkout_id)
                                .cloned();
                            if !emitted || previous != next {
                                emitted = true;
                                previous = next.clone();
                                let value =
                                    serde_json::to_value(zeron_proto::WorkspaceGitStatusFrame {
                                        status: next,
                                    })
                                    .ok()?;
                                return Some((value, (rx, checkout_id, previous, emitted)));
                            }
                        }
                    },
                );
                Ok(RpcReply::Stream(stream.boxed()))
            }
            methods::WATCH_CHECKOUT_CHANGE_REQUEST => {
                let p: CheckoutChangeRequestParams = parse_params(params)?;
                let cwd = self.change_request_root(&p.cwd).await?;
                let stream = self
                    .change_requests
                    .watch_for_branch(&cwd, p.branch.as_deref())
                    .await
                    .map_err(|error| RpcError::Failed(error.to_string()))?
                    .filter_map(|status| async move { serde_json::to_value(status).ok() });
                Ok(RpcReply::Stream(stream.boxed()))
            }
            // One-shot scoped capture for the Changes pane: `branch` diffs the
            // working tree against merge-base(baseRef, HEAD); `turn` diffs the
            // turn-start tree snapshot against the current tree; anything else
            // is the plain working-tree capture.
            methods::GET_CHECKOUT_DIFF => {
                // Keep the scoped-diff future off the dispatcher's stack. The
                // per-commit path adds another nested git-capture future.
                Box::pin(async move {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct P {
                        cwd: String,
                        #[serde(default)]
                        mode: String,
                        base_ref: Option<String>,
                        chat_id: Option<String>,
                        commit_sha: Option<String>,
                    }
                    let p: P = parse_params(params)?;
                    let identity = self
                        .repos
                        .checkout_identity(std::path::Path::new(&p.cwd))
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    let root = identity.root.as_path();
                    let snapshot = match p.mode.as_str() {
                        "branch" => {
                            let base_ref = p
                                .base_ref
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("baseRef required".into()))?;
                            let base = crate::diff_sync::merge_base(root, base_ref)
                                .await
                                .map_err(|e| RpcError::Failed(e.to_string()))?;
                            crate::diff_sync::capture_diff_against(&self.repos, root, Some(&base))
                                .await
                        }
                        // One commit's own changes (History → per-commit tab):
                        // parent (or the empty tree) vs the commit itself.
                        "commit" => {
                            let sha = p
                                .commit_sha
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("commitSha required".into()))?;
                            crate::diff_sync::capture_commit_diff(&self.repos, root, sha).await
                        }
                        "turn" => {
                            let chat_id = p
                                .chat_id
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("chatId required".into()))?;
                            let snapshot = self
                                .diff_sync
                                .turn_snapshot(chat_id)
                                .filter(|s| s.root == identity.root)
                                .ok_or_else(|| RpcError::Failed("no turn recorded".into()))?;
                            crate::diff_sync::capture_turn_diff(&self.repos, root, &snapshot.tree)
                                .await
                        }
                        _ => crate::diff_sync::capture_diff(&self.repos, root).await,
                    }
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                    RpcReply::value(&zeron_proto::CheckoutDiff {
                        checkout_id: identity.id,
                        device_id: self.doc_host.device_id().to_string(),
                        cwd: identity.root.to_string_lossy().to_string(),
                        patch: snapshot.patch,
                        files: snapshot.files,
                        additions: snapshot.additions,
                        deletions: snapshot.deletions,
                        truncated: snapshot.truncated,
                        checksum: snapshot.checksum,
                        updated_at: chrono::Utc::now(),
                    })
                })
                .await
            }
            methods::DISCARD_WORKING_TREE => {
                // This destructive branch performs several nested filesystem
                // futures. Box it so unrelated RPC calls do not inherit that
                // state in the already-large dispatcher stack frame.
                Box::pin(async move {
                    let p: DiscardWorkingTreeParams = parse_params(params)?;
                    let chat = self
                        .workspace
                        .chat(&p.chat_id)
                        .map_err(|e| RpcError::Failed(e.to_string()))?
                        .ok_or_else(|| RpcError::Failed("chat not found".into()))?;
                    if chat.device_id != self.doc_host.device_id() {
                        return Err(RpcError::Failed("chat is not hosted by this device".into()));
                    }
                    let cwd = chat
                        .cwd
                        .as_deref()
                        .ok_or_else(|| RpcError::Failed("chat has no checkout".into()))?;
                    let identity = self
                        .repos
                        .checkout_identity(std::path::Path::new(cwd))
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    if identity.id != p.checkout_id {
                        return Err(RpcError::Failed(
                            "chat checkout changed since the confirmation was opened".into(),
                        ));
                    }

                    // Refuse the mutation when any local chat on this exact
                    // checkout has a live run. We never interrupt an agent as a
                    // side effect of discarding files.
                    let chats = self.workspace.watch_chats().borrow().clone();
                    for candidate in chats {
                        if candidate.device_id != self.doc_host.device_id() {
                            continue;
                        }
                        let same_checkout =
                            if candidate.checkout_id.as_deref() == Some(identity.id.as_str()) {
                                true
                            } else if let Some(candidate_cwd) = candidate.cwd.as_deref() {
                                self.repos
                                    .checkout_identity(std::path::Path::new(candidate_cwd))
                                    .await
                                    .is_ok_and(|candidate_identity| {
                                        candidate_identity.id == identity.id
                                    })
                            } else {
                                false
                            };
                        if same_checkout
                            && self
                                .sessions
                                .session_status(&candidate.id)
                                .is_some_and(|session| {
                                    matches!(
                                        session.status,
                                        zeron_proto::SessionStatus::Working
                                            | zeron_proto::SessionStatus::AwaitingInput
                                    )
                                })
                        {
                            return Err(RpcError::Failed(
                                "an agent is active in this working tree".into(),
                            ));
                        }
                    }

                    let snapshot = self
                        .diff_sync
                        .discard_working_tree(&identity.id, &p.expected_checksum)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    RpcReply::value(&serde_json::json!({
                        "ok": true,
                        "checksum": snapshot.checksum,
                    }))
                })
                .await
            }
            methods::GET_CHECKOUT_FILE_DIFF_TEXT => {
                // This branch contains several large nested async futures. Keep it
                // behind an allocation so every unrelated RPC does not carry that
                // state in `EngineRpc::handle`'s stack frame.
                Box::pin(async move {
                    let p: zeron_proto::GetCheckoutFileDiffTextRequest = parse_params(params)?;
                    let identity =
                        Box::pin(self.repos.checkout_identity(std::path::Path::new(&p.cwd)))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                    if identity.id != p.checkout_id {
                        return Err(RpcError::Failed("checkoutId does not match cwd".into()));
                    }
                    let root = identity.root.as_path();
                    let (snapshot, base, target) = match p.mode.as_str() {
                        "branch" => {
                            let base_ref = p
                                .base_ref
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("baseRef required".into()))?;
                            let base = Box::pin(crate::diff_sync::merge_base(root, base_ref))
                                .await
                                .map_err(|error| RpcError::Failed(error.to_string()))?;
                            let snapshot = Box::pin(crate::diff_sync::capture_diff_against(
                                &self.repos,
                                root,
                                Some(&base),
                            ))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, base, None)
                        }
                        "commit" => {
                            let sha = p
                                .commit_sha
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("commitSha required".into()))?;
                            let base =
                                Box::pin(crate::diff_sync::commit_diff_base(root, sha)).await;
                            let snapshot = Box::pin(crate::diff_sync::capture_commit_diff(
                                &self.repos,
                                root,
                                sha,
                            ))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, base, Some(sha.to_string()))
                        }
                        "turn" => {
                            let chat_id = p
                                .chat_id
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("chatId required".into()))?;
                            let turn = self
                                .diff_sync
                                .turn_snapshot(chat_id)
                                .filter(|snapshot| snapshot.root == identity.root)
                                .ok_or_else(|| RpcError::Failed("no turn recorded".into()))?;
                            let snapshot = Box::pin(crate::diff_sync::capture_turn_diff(
                                &self.repos,
                                root,
                                &turn.tree,
                            ))
                            .await
                            .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, turn.tree, None)
                        }
                        _ => {
                            let base = Box::pin(crate::diff_sync::working_diff_base(root))
                                .await
                                .map_err(|error| RpcError::Failed(error.to_string()))?;
                            let snapshot =
                                Box::pin(crate::diff_sync::capture_diff(&self.repos, root))
                                    .await
                                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                            (snapshot, base, None)
                        }
                    };
                    let stale = || zeron_proto::CheckoutFileDiffText {
                        diff_checksum: p.diff_checksum.clone(),
                        old_text: None,
                        new_text: None,
                        old_content_hash: None,
                        new_content_hash: None,
                        binary: false,
                        truncated: false,
                        stale: true,
                    };
                    if snapshot.checksum != p.diff_checksum {
                        return RpcReply::value(&stale());
                    }
                    let file = snapshot
                        .files
                        .iter()
                        .find(|file| file.path == p.path)
                        .ok_or_else(|| {
                            RpcError::Failed("path is not part of diff snapshot".into())
                        })?;
                    let pair = Box::pin(crate::diff_sync::read_diff_file_text_at(
                        root,
                        &base,
                        target.as_deref(),
                        file,
                    ))
                    .await
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                    let current = match p.mode.as_str() {
                        "branch" => {
                            Box::pin(crate::diff_sync::capture_diff_against(
                                &self.repos,
                                root,
                                Some(&base),
                            ))
                            .await
                        }
                        "turn" => {
                            Box::pin(crate::diff_sync::capture_turn_diff(
                                &self.repos,
                                root,
                                &base,
                            ))
                            .await
                        }
                        "commit" => {
                            let sha = p
                                .commit_sha
                                .as_deref()
                                .ok_or_else(|| RpcError::Failed("commitSha required".into()))?;
                            Box::pin(crate::diff_sync::capture_commit_diff(
                                &self.repos,
                                root,
                                sha,
                            ))
                            .await
                        }
                        _ => Box::pin(crate::diff_sync::capture_diff(&self.repos, root)).await,
                    }
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                    if current.checksum != p.diff_checksum {
                        return RpcReply::value(&stale());
                    }
                    RpcReply::value(&zeron_proto::CheckoutFileDiffText {
                        diff_checksum: p.diff_checksum,
                        old_text: pair.old_text,
                        new_text: pair.new_text,
                        old_content_hash: pair.old_content_hash,
                        new_content_hash: pair.new_content_hash,
                        binary: pair.binary,
                        truncated: pair.truncated,
                        stale: false,
                    })
                })
                .await
            }
            methods::LIST_REPOS => RpcReply::value(&self.repos.list().await),
            methods::ADD_REPO => {
                #[derive(Deserialize)]
                struct P {
                    path: String,
                }
                let p: P = parse_params(params)?;
                let repo = self
                    .repos
                    .add(&p.path)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&repo)
            }
            methods::CLONE_REPO => {
                #[derive(Deserialize)]
                struct P {
                    url: String,
                }
                let p: P = parse_params(params)?;
                let repo = self
                    .repos
                    .clone_repo(&p.url)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&repo)
            }
            methods::CREATE_REPO => {
                #[derive(Deserialize)]
                struct P {
                    name: String,
                }
                let p: P = parse_params(params)?;
                let repo = self
                    .repos
                    .create(&p.name)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&repo)
            }
            methods::LIST_BRANCHES => {
                let p: RepoPathParams = parse_params(params)?;
                let branches = self
                    .repos
                    .branches(std::path::Path::new(&p.repo_path))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&branches)
            }
            methods::LIST_REFS => {
                let p: RepoPathParams = parse_params(params)?;
                let refs = self
                    .repos
                    .refs(std::path::Path::new(&p.repo_path))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&refs)
            }
            methods::LIST_GIT_HISTORY => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    cwd: String,
                    #[serde(default)]
                    cursor: usize,
                    #[serde(default = "default_git_history_limit")]
                    limit: usize,
                }
                fn default_git_history_limit() -> usize {
                    crate::repos::GIT_HISTORY_DEFAULT_LIMIT
                }
                let p: P = parse_params(params)?;
                let history = self
                    .repos
                    .history(std::path::Path::new(&p.cwd), p.cursor, p.limit)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&history)
            }
            methods::SEARCH_GIT_HISTORY => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    cwd: String,
                    query: String,
                    #[serde(default)]
                    cursor: usize,
                    #[serde(default = "default_git_history_search_limit")]
                    limit: usize,
                }
                fn default_git_history_search_limit() -> usize {
                    crate::repos::GIT_HISTORY_DEFAULT_LIMIT
                }
                let p: P = parse_params(params)?;
                let history = self
                    .repos
                    .search_history(std::path::Path::new(&p.cwd), &p.query, p.cursor, p.limit)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&history)
            }
            methods::RESOLVE_GIT_AVATARS => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    cwd: String,
                    authors: Vec<GitAvatarAuthor>,
                    #[serde(default)]
                    cursor: usize,
                    #[serde(default = "default_git_avatar_limit")]
                    limit: usize,
                }
                #[derive(Deserialize)]
                struct GitAvatarAuthor {
                    sha: String,
                    email: String,
                }
                fn default_git_avatar_limit() -> usize {
                    crate::repos::GIT_HISTORY_DEFAULT_LIMIT
                }
                let p: P = parse_params(params)?;
                let authors: Vec<_> = p
                    .authors
                    .into_iter()
                    .take(crate::repos::GIT_HISTORY_MAX_LIMIT)
                    .filter(|author| author.sha.len() <= 64 && author.email.len() <= 512)
                    .map(|author| (author.sha, author.email))
                    .collect();
                let avatar_paths = self
                    .repos
                    .history_avatar_urls(std::path::Path::new(&p.cwd), &authors, p.cursor, p.limit)
                    .await;
                let mut avatars = std::collections::HashMap::new();
                for (email, path) in avatar_paths {
                    if let Ok(bytes) = tokio::fs::read(path).await {
                        avatars.insert(
                            email,
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                        );
                    }
                }
                RpcReply::value(&avatars)
            }
            methods::FETCH_ALL => {
                let p: RepoPathParams = parse_params(params)?;
                self.repos
                    .fetch_all(std::path::Path::new(&p.repo_path))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                // Remote refs are repository state too. Force the checkout
                // watchers to publish a fresh snapshot instead of waiting for
                // the repair tick (some platforms do not report packed-refs).
                self.diff_sync.sync_all();
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::SWITCH_REF => {
                let p: SwitchRefParams = parse_params(params)?;
                let branch = self
                    .repos
                    .switch_ref(std::path::Path::new(&p.repo_path), &p.ref_name)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "branch": branch }))
            }
            methods::LIST_FOLDERS => {
                let p: ListFoldersParams = parse_params(params)?;
                let listing = self
                    .repos
                    .list_folders(p.path)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&listing)
            }
            methods::LIST_DRIVES => {
                let drives = self
                    .repos
                    .list_drives()
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&zeron_proto::DriveListing { drives })
            }
            methods::SEARCH_FILES => {
                let p: FileSearchParams = parse_params(params)?;
                if p.query.chars().count() > 256 {
                    return Err(RpcError::BadParams(
                        "SearchFiles query must not exceed 256 characters".into(),
                    ));
                }
                let matches = tokio::time::timeout(FILE_SEARCH_RPC_TIMEOUT, async {
                    let root = self.file_search_root(&p).await?;
                    let featured_paths = p
                        .chat_id
                        .as_deref()
                        .filter(|_| p.query.is_empty())
                        .map(|chat_id| self.featured_file_paths(chat_id))
                        .unwrap_or_default();
                    self.repos
                        .search_files(root, p.query, featured_paths)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))
                })
                .await
                .map_err(|_| RpcError::Failed("file search timed out".into()))??;
                RpcReply::value(&matches)
            }
            methods::LIST_WORKSPACE_DIRECTORY => {
                let request: zeron_proto::ListWorkspaceDirectoryRequest = parse_params(params)?;
                let page = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.list_directory(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace directory listing timed out".into()))?
                .map_err(RpcError::from)?;
                RpcReply::value(&page)
            }
            methods::SEARCH_WORKSPACE_FILES => {
                let request: zeron_proto::SearchWorkspaceFilesRequest = parse_params(params)?;
                let matches = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.search(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace file search timed out".into()))?
                .map_err(RpcError::from)?;
                RpcReply::value(&matches)
            }
            methods::READ_WORKSPACE_IMAGE => {
                let request: zeron_proto::ReadWorkspaceImageRequest = parse_params(params)?;
                let chunk = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.read_image(request),
                )
                .await
                .map_err(|_| RpcError::Failed("Workspace image read timed out".into()))?
                .map_err(RpcError::from)?;
                RpcReply::value(&chunk)
            }
            methods::READ_WORKSPACE_FILE => {
                let request: zeron_proto::ReadWorkspaceFileRequest = parse_params(params)?;
                let file = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.read_file(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace file read timed out".into()))?
                .map_err(RpcError::from)?;
                RpcReply::value(&file)
            }
            methods::WRITE_WORKSPACE_FILE => {
                let request: zeron_proto::WriteWorkspaceFileRequest = parse_params(params)?;
                let outcome = tokio::time::timeout(
                    crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT,
                    self.workspace_files.write_file(request),
                )
                .await
                .map_err(|_| RpcError::Failed("workspace file write timed out".into()))?
                .map_err(RpcError::from)?;
                RpcReply::value(&outcome)
            }
            methods::WATCH_WORKSPACE_FILES => {
                let request: zeron_proto::WatchWorkspaceFilesRequest = parse_params(params)?;
                let subscription = self
                    .workspace_files
                    .watch_files(request)
                    .await
                    .map_err(RpcError::from)?;
                let stream = futures::stream::unfold(subscription, |mut subscription| async move {
                    let changes = subscription.recv().await?;
                    let value = serde_json::to_value(changes).ok()?;
                    Some((value, subscription))
                });
                Ok(RpcReply::Stream(stream.boxed()))
            }
            methods::CREATE_WORKTREE => {
                let p: CreateWorktreeParams = parse_params(params)?;
                let setup_space = match p.space_id.as_deref() {
                    Some(space_id) => {
                        let space = self.local_project_action_space(space_id)?;
                        let space_root = std::fs::canonicalize(&space.path)
                            .map_err(|_| RpcError::Failed("Project root is unavailable".into()))?;
                        let repo_root = std::fs::canonicalize(&p.repo_path).map_err(|_| {
                            RpcError::Failed("Worktree repository is unavailable".into())
                        })?;
                        if space_root != repo_root {
                            return Err(RpcError::Failed(
                                "Worktree repository does not match project space".into(),
                            ));
                        }
                        Some((space, space_root))
                    }
                    None => None,
                };
                let worktree = self
                    .repos
                    .create_worktree(std::path::Path::new(&p.repo_path), &p.branch)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let mut outcome = CreateWorktreeOutcome {
                    worktree,
                    setup_action: None,
                    setup_error: None,
                };
                if let Some((space, project_root)) = setup_space {
                    match self
                        .project_actions
                        .setup_action(&space.id, std::path::Path::new(&space.path))
                    {
                        Ok(Some(action)) => {
                            let worktree_root = std::fs::canonicalize(&outcome.worktree.path)
                                .unwrap_or_else(|_| outcome.worktree.path.clone().into());
                            match crate::project_actions::launch_project_setup_action(
                                &self.terminals,
                                &action,
                                &project_root,
                                &worktree_root,
                                80,
                                24,
                            ) {
                                Ok(run) => outcome.setup_action = Some(run),
                                Err(err) => {
                                    tracing::warn!(
                                        space_id = %space.id,
                                        worktree = %outcome.worktree.path,
                                        error = %err,
                                        "failed to start project setup Action"
                                    );
                                    outcome.setup_error = Some(err.to_string());
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(
                                space_id = %space.id,
                                error = %err,
                                "failed to resolve project setup Action"
                            );
                            outcome.setup_error = Some(err.to_string());
                        }
                    }
                }
                RpcReply::value(&outcome)
            }
            methods::DELETE_WORKTREE => {
                let p: DeleteWorktreeParams = parse_params(params)?;
                self.repos
                    .delete_worktree(
                        std::path::Path::new(&p.repo_path),
                        std::path::Path::new(&p.worktree_path),
                    )
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::LIST_PROJECT_ACTIONS => {
                let p: ListProjectActionsParams = parse_params(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let actions = self.project_actions.clone();
                // Snapshots discover repository files; keep all filesystem work
                // (including mutation persistence below) off the async worker.
                let snapshot = tokio::task::spawn_blocking(move || {
                    actions.snapshot(&space.id, std::path::Path::new(&space.path))
                })
                .await
                .map_err(|err| RpcError::Failed(err.to_string()))?
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::UPSERT_PROJECT_ACTION => {
                let p: UpsertProjectActionParams = parse_params(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let actions = self.project_actions.clone();
                let snapshot = tokio::task::spawn_blocking(move || {
                    actions.upsert(
                        &space.id,
                        std::path::Path::new(&space.path),
                        p.action_id.as_deref(),
                        p.action,
                    )
                })
                .await
                .map_err(|err| RpcError::Failed(err.to_string()))?
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::DELETE_PROJECT_ACTION => {
                let p: DeleteProjectActionParams = parse_params(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let actions = self.project_actions.clone();
                let snapshot = tokio::task::spawn_blocking(move || {
                    actions.delete(&space.id, std::path::Path::new(&space.path), &p.action_id)
                })
                .await
                .map_err(|err| RpcError::Failed(err.to_string()))?
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::RUN_PROJECT_ACTION => {
                let p: RunProjectActionParams = parse_params(params)?;
                let space = self.local_project_action_space(&p.space_id)?;
                let chat = self
                    .workspace
                    .chat(&p.chat_id)
                    .map_err(|err| RpcError::Failed(err.to_string()))?
                    .ok_or_else(|| RpcError::Failed("Project chat not found".into()))?;
                if chat.device_id != self.doc_host.device_id() {
                    return Err(RpcError::Failed(
                        "Project chat belongs to another device".into(),
                    ));
                }
                if chat.space_id.as_deref() != Some(space.id.as_str()) {
                    return Err(RpcError::Failed(
                        "Project chat belongs to another space".into(),
                    ));
                }
                let cwd = chat
                    .cwd
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| RpcError::Failed("Project chat has no checkout".into()))?;
                let checkout = self
                    .repos
                    .workspace_checkout(std::path::Path::new(&space.path), &cwd)
                    .await
                    .ok_or_else(|| {
                        RpcError::Failed("Project chat checkout is unavailable".into())
                    })?;
                let project_root = std::fs::canonicalize(&space.path)
                    .map_err(|_| RpcError::Failed("Project root is unavailable".into()))?;
                let action = self
                    .project_actions
                    .action(&space.id, std::path::Path::new(&space.path), &p.action_id)
                    .map_err(|err| RpcError::Failed(err.to_string()))?
                    .ok_or_else(|| RpcError::Failed("Project action not found".into()))?;
                let run = crate::project_actions::launch_project_action(
                    &self.terminals,
                    &action,
                    &project_root,
                    &checkout,
                    p.cols,
                    p.rows,
                )
                .map_err(|err| RpcError::Failed(err.to_string()))?;
                RpcReply::value(&run)
            }
            methods::OPEN_TERMINAL => {
                let p: OpenTerminalParams = parse_params(params)?;
                // Prefer an explicit real path (new-chat canvas has no row
                // yet). `space-canvas:{spaceId}` names the selected project
                // so a missing/tilde cwd still lands in that folder.
                let chat_cwd = self
                    .workspace
                    .chat(&p.chat_id)
                    .ok()
                    .flatten()
                    .and_then(|chat| chat.cwd);
                let space_cwd = canvas_space_id(&p.chat_id).and_then(|space_id| {
                    self.workspace
                        .space(space_id)
                        .ok()
                        .flatten()
                        .map(|space| space.path)
                });
                let cwd = resolve_open_terminal_cwd(p.cwd, chat_cwd, space_cwd)
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                let session = self
                    .terminals
                    .open(&cwd, p.cols, p.rows)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&session)
            }
            methods::SUBSCRIBE_TERMINAL => {
                let p: SubscribeTerminalParams = parse_params(params)?;
                let rx = self
                    .terminals
                    .subscribe(&p.terminal_id, p.after_seq)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                let stream = futures::stream::unfold(rx, |mut rx| async move {
                    let event = rx.recv().await?;
                    let value = serde_json::to_value(&event).ok()?;
                    Some((value, rx))
                });
                Ok(RpcReply::Stream(stream.boxed()))
            }
            methods::WRITE_TERMINAL => {
                let p: WriteTerminalParams = parse_params(params)?;
                self.terminals
                    .write(&p.terminal_id, &p.data)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::RESIZE_TERMINAL => {
                let p: ResizeTerminalParams = parse_params(params)?;
                self.terminals
                    .resize(&p.terminal_id, p.cols, p.rows)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::CLOSE_TERMINAL => {
                let p: TerminalIdParams = parse_params(params)?;
                self.terminals
                    .close(&p.terminal_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::LIST_AGENT_ACCOUNTS => {
                let p: ListAgentAccountsParams = parse_params(params)?;
                let snapshot = self
                    .agent_accounts
                    .list(p.force_usage.unwrap_or(false))
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::ACTIVATE_AGENT_ACCOUNT => {
                let p: AgentAccountParams = parse_params(params)?;
                let snapshot = self
                    .agent_accounts
                    .activate(p.harness, &p.account_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::FORGET_AGENT_ACCOUNT => {
                let p: AgentAccountParams = parse_params(params)?;
                let snapshot = self
                    .agent_accounts
                    .forget(p.harness, &p.account_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::START_AGENT_LOGIN => {
                let p: StartAgentLoginParams = parse_params(params)?;
                // A requester naming this device is no remote login at all:
                // publishing a callback route for ourselves would be a no-op
                // at best, so never register one.
                let own_id = self.doc_host.device_id();
                let requester = p
                    .requester_device_id
                    .as_deref()
                    .filter(|requester| !requester.is_empty() && *requester != own_id);
                let start = self
                    .agent_accounts
                    .start_login_with(p.harness, p.provider.as_deref(), requester)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&start)
            }
            methods::COMPLETE_AGENT_LOGIN => {
                let p: CompleteAgentLoginParams = parse_params(params)?;
                let snapshot = self
                    .agent_accounts
                    .complete_login(&p.login_id, &p.code)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&snapshot)
            }
            methods::POLL_AGENT_LOGIN => {
                let p: LoginIdParams = parse_params(params)?;
                let poll = self
                    .agent_accounts
                    .poll_login(&p.login_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&poll)
            }
            methods::CANCEL_AGENT_LOGIN => {
                let p: LoginIdParams = parse_params(params)?;
                self.agent_accounts.cancel_login(&p.login_id);
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::UPLOAD_CHUNK => {
                let p: UploadChunkParams = parse_params(params)?;
                self.uploads
                    .append(&p.upload_id, &p.data, p.seq)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "ok": true }))
            }
            methods::UPLOAD_COMMIT => {
                let p: UploadCommitParams = parse_params(params)?;
                let path = self
                    .uploads
                    .commit(&p.upload_id, &p.file_name)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                // Bytes just landed on this device: any command deferred on
                // them (queued-attachment refs) is executable NOW.
                self.doc_host.kick_drains();
                RpcReply::value(&serde_json::json!({ "path": path }))
            }
            methods::READ_ATTACHMENT_CHUNK => {
                let p: ReadAttachmentChunkParams = parse_params(params)?;
                // Path jail: the uploads dir plus every workspace-known chat cwd.
                let roots: Vec<std::path::PathBuf> = self
                    .workspace
                    .read_chats()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|chat| chat.cwd)
                    .map(std::path::PathBuf::from)
                    .collect();
                let chunk = self
                    .uploads
                    .read_chunk(&p.path, p.offset, &roots)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&chunk)
            }
            methods::FETCH_TOOL_BLOB => {
                let p: FetchToolBlobParams = parse_params(params)?;
                let text = self
                    .doc_host
                    .fetch_tool_blob(&p.blob_ref)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&serde_json::json!({ "text": text }))
            }
            other => Err(RpcError::UnknownMethod(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each subprocess has private HOME/PATH/overrides, avoiding process-global test races.
    #[cfg(unix)]
    async fn installer_rpc_fixture(mode: &str) {
        use std::{os::unix::fs::PermissionsExt, sync::Arc};
        if std::env::var_os("ZERON_INSTALL_FIXTURE_CHILD").is_none() {
            let root = tempfile::tempdir().unwrap();
            let bin = root.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            let script = match mode {
                "success" => {
                    "test -z \"$ZERON_INSTALL_FIXTURE_CHILD\" && test -z \"$CLAUDECODE\" && printf '#!/bin/sh\\necho 99.0.0\\n' > \"$CODEX_EXECUTABLE\" && /bin/chmod +x \"$CODEX_EXECUTABLE\""
                }
                "failure" => "echo 'fixture failure api_key=private' >&2; exit 7",
                "missing" => "exit 0",
                "cancel" => "echo ready > \"$READY_FILE\"; sleep 60",
                "npm" => "npm install -g @openai/codex",
                _ => unreachable!(),
            };
            let test = format!("rpc::tests::installer_rpc_{mode}");
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &test, "--nocapture", "--include-ignored"])
                .env("ZERON_INSTALL_FIXTURE_CHILD", root.path())
                .env("ZERON_INSTALLER_COMMAND_CODEX", script)
                .env("ZERON_NO_LOGIN_SHELL", "1")
                .env("HOME", root.path())
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("CODEX_EXECUTABLE", bin.join("codex"))
                .env("CLAUDECODE", "nested-test")
                .env("READY_FILE", root.path().join("ready"))
                .env("npm_config_prefix", root.path())
                .env("npm_config_cache", root.path().join("npm-cache"))
                .env(
                    "PATH",
                    std::env::join_paths(
                        std::iter::once(bin)
                            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                    )
                    .unwrap(),
                )
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            println!("{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        let root =
            std::path::PathBuf::from(std::env::var_os("ZERON_INSTALL_FIXTURE_CHILD").unwrap());
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(zeron_harness::CodexHarness::new()));
        let core = crate::EngineCore::assemble(
            &root.join("engine"),
            registry.clone(),
            HarnessId::Codex,
            None,
        )
        .unwrap();
        let rpc = core.rpc_service();
        let params = serde_json::json!({"harness": "codex"});
        assert!(!registry.descriptors()[0].installed);
        assert_eq!(registry.descriptors()[0].enabled, Some(false));
        let result = if mode == "cancel" {
            let rpc = rpc.clone();
            let task = tokio::spawn(async move {
                rpc.handle(
                    methods::INSTALL_HARNESS,
                    serde_json::json!({"harness": "codex"}),
                )
                .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !root.join("ready").exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            // A second service for the same device must share the in-flight guard.
            let other = core.rpc_service();
            assert!(
                matches!(other.handle(methods::INSTALL_HARNESS, params.clone()).await, Err(RpcError::Failed(e)) if e == "already installing")
            );
            other.handle(methods::CANCEL_INSTALL, params).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
        } else {
            rpc.handle(methods::INSTALL_HARNESS, params).await
        };
        match mode {
            "success" | "npm" => {
                let RpcReply::Value(value) = result.unwrap() else {
                    panic!("expected descriptors");
                };
                let list: Vec<crate::registry::HarnessDescriptor> =
                    serde_json::from_value(value).unwrap();
                assert!(list[0].installed);
                assert_eq!(list[0].enabled, Some(true));
                assert!(list[0].can_install);
                let path = root.join("bin/codex");
                assert!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o111 != 0);
                println!(
                    "InstallHarness ({mode}): installed=false/enabled=false -> installed=true/enabled=true; {}",
                    path.display()
                );
            }
            "failure" => assert!(
                matches!(result, Err(RpcError::Failed(e)) if e.contains("fixture failure") && e.contains("[REDACTED]") && !e.contains("private"))
            ),
            "missing" => assert!(
                matches!(result, Err(RpcError::Failed(e)) if e.contains("installer finished but `codex` was not found on PATH"))
            ),
            "cancel" => {
                assert!(matches!(result, Err(RpcError::Failed(e)) if e.contains("cancelled")));
                assert!(registry.installs.begin(HarnessId::Codex).is_ok());
            }
            _ => unreachable!(),
        }
        assert!(forwardable(methods::CANCEL_INSTALL));
        core.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_success() {
        installer_rpc_fixture("success").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_failure() {
        installer_rpc_fixture("failure").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_missing() {
        installer_rpc_fixture("missing").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_rpc_cancel() {
        installer_rpc_fixture("cancel").await;
    }
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "downloads the official npm package into an isolated temporary prefix"]
    async fn installer_rpc_npm() {
        installer_rpc_fixture("npm").await;
    }

    #[tokio::test]
    async fn explicit_install_rpc_verifies_archive_and_refreshes_descriptors() {
        use sha2::{Digest, Sha512};
        use std::io::Write;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use zeron_harness::archive_install::{ArchivePin, ensure_installed, installed_entry};
        if std::env::var_os("ZERON_INSTALL_RPC_TEST").is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "rpc::tests::explicit_install_rpc_verifies_archive_and_refreshes_descriptors",
                    "--nocapture",
                ])
                .env("ZERON_INSTALL_RPC_TEST", "1")
                .env("ZERON_ADAPTERS_DIR", root.path())
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        if !zeron_harness::acp::can_install(HarnessId::Antigravity) {
            return;
        }
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("server", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let digest = Box::leak(format!("{:x}", Sha512::digest(&bytes)).into_boxed_str());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Box::leak(
            format!("http://{}/archive.zip", listener.local_addr().unwrap()).into_boxed_str(),
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            let mut buf = [0; 4096];
            while !headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buf).await.unwrap();
                assert!(count > 0, "archive request closed before its headers");
                headers.extend_from_slice(&buf[..count]);
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(&bytes).await.unwrap();
        });
        let pin = ArchivePin {
            name: "explicit-install-test",
            version: "1",
            url,
            entry: "server",
            sha512: digest,
        };
        let registry = HarnessRegistry::new();
        let descriptor = serde_json::from_value(serde_json::json!({
            "id": "antigravity", "name": "Antigravity", "supportsSteering": true,
            "steeringMode": "turn-boundary", "reasoningLevels": [], "installed": false
        }))
        .unwrap();
        registry.register_lazy(
            descriptor,
            Box::new(move || installed_entry(&pin).is_some()),
            Box::new(|| panic!("installation must not spawn the harness")),
        );
        assert!(!registry.descriptors()[0].installed);
        let result = install_harness_with(&registry, HarnessId::Antigravity, || async {
            ensure_installed(pin, "Test adapter").await.map(|_| ())
        })
        .await
        .unwrap();
        assert!(result[0].installed);
        assert_eq!(result[0].enabled, Some(true));
        assert!(result[0].can_install);
        assert!(installed_entry(&pin).unwrap().is_file());
        server.await.unwrap();
        // The verified marker makes another explicit install idempotent, even
        // after the archive server has stopped.
        install_harness_with(&registry, HarnessId::Antigravity, || async {
            ensure_installed(pin, "Test adapter").await.map(|_| ())
        })
        .await
        .unwrap();
        assert!(
            install_harness_with(&registry, HarnessId::Mock, || async {
                panic!("unsupported harness must not invoke an installer")
            })
            .await
            .is_err()
        );
        assert!(forwardable(methods::INSTALL_HARNESS));
        assert_eq!(
            forward_deadline(methods::INSTALL_HARNESS),
            std::time::Duration::from_secs(15 * 60)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn local_opening_tail_arrives_before_full_mirror_and_keeps_all_history() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "viewer".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("whale").unwrap();
        handle
            .doc()
            .push_message(&zeron_doc::SessionMessageEntry {
                id: "turn".into(),
                role: zeron_doc::MessageRole::Assistant,
                parts: (0..500)
                    .map(|i| zeron_doc::MessagePart::Text {
                        id: format!("part-{i}"),
                        text: "local text".into(),
                    })
                    .collect(),
                created_at: 0,
                device_id: "host".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        // Hold publication blocked: the opening must not await the full mirror.
        let held = handle.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = std::thread::spawn(move || {
            held.import_transcript(|| {
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        });
        locked_rx.recv().unwrap();
        let opening = tokio::time::timeout(
            Duration::from_secs(2),
            opening_doc_messages_stream(host.clone(), "whale".into()),
        )
        .await;
        release_tx.send(()).unwrap();
        blocker.join().unwrap();
        let mut stream = opening
            .expect("first paint must not wait for publication")
            .unwrap();
        let preview = stream.next().await.unwrap();
        assert_eq!(preview["historyPending"], true);
        assert_eq!(preview["reset"][0]["parts"].as_array().unwrap().len(), 128);
        assert_eq!(preview["reset"][0]["parts"][0]["id"], "part-372");
        // Changes between preview and subscribe must appear in the full reset.
        handle
            .write_user_message("arrived", "new local message", 1)
            .unwrap();
        let full = stream.next().await.unwrap();
        assert!(full.get("historyPending").is_none());
        assert_eq!(full["reset"][0]["parts"].as_array().unwrap().len(), 500);
        assert_eq!(full["reset"][1]["id"], "arrived");
        handle
            .write_user_message("live", "after attach", 2)
            .unwrap();
        let live = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap();
        let mut entries = Vec::new();
        for value in [full, live] {
            let update: zeron_doc::TranscriptUpdate = serde_json::from_value(value).unwrap();
            zeron_doc::apply_transcript_frame(&mut entries, update.frame).unwrap();
        }
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.last().unwrap().id, "live");
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn antigravity_disable_does_not_launch_the_server() {
        let registry = HarnessRegistry::new();
        let executable = std::env::current_exe().unwrap();
        registry.register(std::sync::Arc::new(
            zeron_harness::AcpHarness::grok().with_executable(executable.clone()),
        ));
        registry.register(std::sync::Arc::new(
            zeron_harness::AcpHarness::antigravity().with_executable(executable),
        ));
        registry.set_enabled(HarnessId::Antigravity, true).unwrap();

        update_harness_enabled(&registry, HarnessId::Antigravity, false)
            .await
            .unwrap();
        assert!(!registry.enabled_set().contains(&HarnessId::Antigravity));
    }

    /// The UI's Switch/Forget calls send `{id, accountId, harness}` (+ optional
    /// `targetDeviceId`); the extra fields must be tolerated, `accountId` wins.
    #[test]
    fn list_models_force_is_optional_and_backward_compatible() {
        let old: ListModelsParams =
            serde_json::from_value(serde_json::json!({"harness":"codex"})).unwrap();
        assert!(!old.force);
        let forced: ListModelsParams =
            serde_json::from_value(serde_json::json!({"harness":"codex","force":true})).unwrap();
        assert!(forced.force);
    }

    #[test]
    fn agent_account_params_accept_ui_shape() {
        let p: AgentAccountParams = parse_params(serde_json::json!({
            "id": "acct-1",
            "accountId": "acct-1",
            "harness": "claude-code",
            "targetDeviceId": "dev-2",
        }))
        .expect("ui param shape");
        assert_eq!(p.account_id, "acct-1");
        assert_eq!(p.harness, HarnessId::ClaudeCode);
    }

    #[test]
    fn sidebar_preferences_mutation_accepts_desktop_wire_shape() {
        let p: MutateParams = parse_params(serde_json::json!({
            "op": "changeSidebarPin",
            "change": {"action":"move","sessionId":"chat-b","before":"chat-a","after":null},
        }))
        .expect("sidebar preferences params");
        assert!(matches!(
            p,
            MutateParams::ChangeSidebarPin { change: zeron_proto::SidebarPinChange::Move { session_id, before, .. } }
                if session_id == "chat-b" && before.as_deref() == Some("chat-a")
        ));
    }

    #[test]
    fn local_device_is_not_forwardable() {
        assert!(!forwardable(methods::LOCAL_DEVICE));
        assert!(!forwardable(methods::FOCUS_CHAT));
        assert!(!forwardable(methods::ENGINE_INFO));
        assert!(!forwardable(methods::ENGINE_READY));
        assert!(forwardable(methods::QUEUE_COMMAND));
        assert!(forwardable(methods::SEARCH_FILES));
        assert!(forwardable(methods::SEARCH_GIT_HISTORY));
        assert!(forwardable(methods::FETCH_ALL));
        assert!(forwardable(methods::RESOLVE_GIT_AVATARS));
        assert!(forwardable(methods::WATCH_CHECKOUT_CHANGE_REQUEST));
        assert!(is_stream_method(methods::WATCH_CHECKOUT_CHANGE_REQUEST));
        assert!(forwardable(methods::DISCARD_WORKING_TREE));
        assert!(forwardable(methods::LIST_WORKSPACE_DIRECTORY));
        assert!(forwardable(methods::SEARCH_WORKSPACE_FILES));
        assert!(forwardable(methods::READ_WORKSPACE_FILE));
        assert!(forwardable(methods::READ_WORKSPACE_IMAGE));
        assert!(forwardable(methods::WRITE_WORKSPACE_FILE));
        assert!(forwardable(methods::WATCH_WORKSPACE_FILES));
        assert!(forwardable(methods::WATCH_WORKSPACE_GIT_STATUS));
        assert!(!is_stream_method(methods::LIST_WORKSPACE_DIRECTORY));
        assert!(!is_stream_method(methods::SEARCH_WORKSPACE_FILES));
        assert!(!is_stream_method(methods::READ_WORKSPACE_FILE));
        assert!(!is_stream_method(methods::WRITE_WORKSPACE_FILE));
        assert!(is_stream_method(methods::WATCH_WORKSPACE_FILES));
        assert!(is_stream_method(methods::WATCH_WORKSPACE_GIT_STATUS));
    }

    /// Every forwardable unary method gets a bounded reply deadline —
    /// interactive calls fail fast, network-bound git/update calls get the
    /// long leash, and nothing awaits forever (the "Sending…" wedge).
    #[test]
    fn forward_deadlines_are_tiered_and_bounded() {
        for method in [methods::LIST_MODELS, methods::LIST_COMMANDS] {
            assert_eq!(
                forward_deadline(method),
                std::time::Duration::from_secs(100)
            );
        }
        use std::time::Duration;
        assert_eq!(
            forward_deadline(methods::CREATE_WORKTREE),
            Duration::from_secs(120)
        );
        assert_eq!(
            forward_deadline(methods::CLONE_REPO),
            Duration::from_secs(15 * 60)
        );
        assert_eq!(
            forward_deadline(methods::LIST_BRANCHES),
            Duration::from_secs(30)
        );
        assert_eq!(
            forward_deadline(methods::QUEUE_COMMAND),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn open_terminal_cwd_prefers_explicit_then_chat_then_space_then_home() {
        let home = crate::repos::session_home_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_open_terminal_cwd(
                Some("/proj".into()),
                Some("/chat".into()),
                Some("/space".into())
            )
            .unwrap(),
            "/proj"
        );
        assert_eq!(
            resolve_open_terminal_cwd(None, Some("/chat".into()), Some("/space".into())).unwrap(),
            "/chat"
        );
        assert_eq!(
            resolve_open_terminal_cwd(Some("~".into()), None, Some("/space".into())).unwrap(),
            "/space",
            "tilde is a fallback, not an override of the selected project"
        );
        assert_eq!(
            resolve_open_terminal_cwd(None, None, Some("/space".into())).unwrap(),
            "/space"
        );
        assert_eq!(resolve_open_terminal_cwd(None, None, None).unwrap(), home);
        assert_eq!(
            resolve_open_terminal_cwd(Some("~".into()), Some("/chat".into()), None).unwrap(),
            "/chat"
        );
        assert_eq!(
            resolve_open_terminal_cwd(Some("  ".into()), Some("/chat".into()), None).unwrap(),
            "/chat"
        );
        assert_eq!(canvas_space_id("space-canvas:s1"), Some("s1"));
        assert_eq!(canvas_space_id("space-canvas:"), None);
        assert_eq!(canvas_space_id("chat-1"), None);
    }

    #[test]
    fn tool_file_paths_keep_workspace_activity_only() {
        assert_eq!(
            tool_file_path(&ToolCall::EditFile {
                path: "src/main.rs".into(),
                old_string: None,
                new_string: None,
            }),
            Some("src/main.rs")
        );
        assert_eq!(
            tool_file_path(&ToolCall::Exec {
                command: "cargo test".into(),
            }),
            None
        );
    }
}

#[cfg(test)]
mod context_usage_tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::Arc;

    #[tokio::test]
    async fn replay_cutoff_travels_with_coalesced_backfill_and_live_content() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use zeron_sync::chat_client::ChatDocSink;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "viewer".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("replay-chat").unwrap();
        let sink = crate::chat2_host::EngineChatSink::new(&handle.doc_arc(), store, "replay-chat")
            .with_handle(Arc::downgrade(&handle));
        let mut stream = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        let first: zeron_doc::TranscriptUpdate =
            serde_json::from_value(stream.next().await.unwrap()).unwrap();
        assert!(first.replay_baseline.unwrap().entries.is_empty());

        let source = zeron_doc::SessionDoc::init("replay-chat").unwrap();
        let append = |id: &str| {
            source
                .push_message(&zeron_doc::SessionMessageEntry {
                    id: id.into(),
                    role: zeron_doc::MessageRole::Assistant,
                    parts: vec![zeron_doc::MessagePart::Text {
                        id: "text".into(),
                        text: id.into(),
                    }],
                    created_at: 0,
                    device_id: "writer".into(),
                    status: Some(zeron_doc::MessageStatus::Streaming),
                    continuation_of: None,
                    duration_ms: None,
                })
                .unwrap()
        };
        append("cached");
        sink.apply_checkpoint(&source.export_snapshot().unwrap(), 0)
            .unwrap();
        let checkpoint: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            checkpoint
                .replay_baseline
                .unwrap()
                .entries
                .contains_key("cached")
        );

        let version = source.doc().oplog_vv();
        append("away");
        sink.apply_replay_row(
            &source
                .doc()
                .export(loro::ExportMode::updates(&version))
                .unwrap(),
            1,
        );
        let version = source.doc().oplog_vv();
        append("live");
        sink.apply_row(
            &source
                .doc()
                .export(loro::ExportMode::updates(&version))
                .unwrap(),
            2,
        );
        // Neither the doc worker nor the RPC consumer ran between these
        // imports. They must not flatten their different presentation origins.
        let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let cutoff = update.replay_baseline.unwrap();
        assert!(cutoff.entries.contains_key("away"));
        assert!(!cutoff.entries.contains_key("live"));
        let mut entries = vec![];
        zeron_doc::apply_transcript_frame(&mut entries, checkpoint.frame).unwrap();
        zeron_doc::apply_transcript_frame(&mut entries, update.frame).unwrap();
        assert_eq!(entries.len(), 3);

        let version = source.doc().oplog_vv();
        append("next-live");
        sink.apply_row(
            &source
                .doc()
                .export(loro::ExportMode::updates(&version))
                .unwrap(),
            3,
        );
        let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            update.replay_baseline.is_none(),
            "live updates must not resend the history watermark"
        );
        let mut reopened = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        let opening: zeron_doc::TranscriptUpdate =
            serde_json::from_value(reopened.next().await.unwrap()).unwrap();
        assert_eq!(
            opening.replay_baseline.unwrap().entries.len(),
            4,
            "reopening includes all existing content as history"
        );
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn replay_metadata_and_backfill_leave_interleaved_local_content_live() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use zeron_sync::chat_client::ChatDocSink;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("interleaved").unwrap();
        let sink = crate::chat2_host::EngineChatSink::new(&handle.doc_arc(), store, "interleaved")
            .with_handle(Arc::downgrade(&handle));
        let mut stream = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        stream.next().await.unwrap();
        let source = zeron_doc::SessionDoc::init("interleaved").unwrap();
        sink.apply_checkpoint(&source.export_snapshot().unwrap(), 0)
            .unwrap();
        let entry = |id: &str| zeron_doc::SessionMessageEntry {
            id: id.into(),
            role: zeron_doc::MessageRole::Assistant,
            parts: vec![zeron_doc::MessagePart::Text {
                id: "text".into(),
                text: id.into(),
            }],
            created_at: 0,
            device_id: "host".into(),
            status: Some(zeron_doc::MessageStatus::Streaming),
            continuation_of: None,
            duration_ms: None,
        };
        handle.doc().push_message(&entry("local-before")).unwrap();
        source.update_context_usage(Some(10), Some(100)).unwrap();
        sink.apply_replay_row(&source.export_snapshot().unwrap(), 1);
        let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            update.replay_baseline.is_none(),
            "metadata must not reset ongoing live animations"
        );
        let mut entries = vec![];
        zeron_doc::apply_transcript_frame(&mut entries, update.frame).unwrap();
        assert_eq!(entries[0].id, "local-before");
        let version = source.doc().oplog_vv();
        source.push_message(&entry("historical")).unwrap();
        handle.doc().push_message(&entry("local-between")).unwrap();
        sink.apply_replay_row(
            &source
                .doc()
                .export(loro::ExportMode::updates(&version))
                .unwrap(),
            2,
        );
        handle.doc().push_message(&entry("local-after")).unwrap();
        let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let baseline = update.replay_baseline.unwrap();
        assert_eq!(baseline.entries.len(), 1);
        assert!(baseline.entries.contains_key("historical"));
        zeron_doc::apply_transcript_frame(&mut entries, update.frame).unwrap();
        assert_eq!(entries.len(), 4);
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn replay_preserves_each_watchers_opening_cutoff_without_consuming_live_text() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use zeron_sync::chat_client::ChatDocSink;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "viewer".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("cached-replay").unwrap();
        let sink =
            crate::chat2_host::EngineChatSink::new(&handle.doc_arc(), store, "cached-replay")
                .with_handle(Arc::downgrade(&handle));
        let source = zeron_doc::SessionDoc::init("cached-replay").unwrap();
        let mut writer = zeron_doc::SegmentWriter::begin(&source, "reply", "host", 0).unwrap();
        let text = |id: &str, value: &str| zeron_doc::MessagePart::Text {
            id: id.into(),
            text: value.into(),
        };
        let cached = text("body", "café histórico");
        writer.sync(&[cached.clone()]).unwrap();
        sink.apply_checkpoint(&source.export_snapshot().unwrap(), 0)
            .unwrap();
        // Cached content exists before the first watcher and never enters
        // the changed-parts tracker. It may not have been painted yet.
        let mut first = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        let opening: zeron_doc::TranscriptUpdate =
            serde_json::from_value(first.next().await.unwrap()).unwrap();
        assert_eq!(
            opening.replay_baseline.unwrap().entries["reply"]["body"],
            "café histórico".len()
        );

        let live = text("body", "café histórico y nuevo");
        writer.sync(&[live.clone()]).unwrap();
        sink.apply_row(&source.export_snapshot().unwrap(), 1);
        let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), first.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(update.replay_baseline.is_none());
        // A later subscriber sees a longer historical prefix, but must not
        // change the first subscriber's ongoing live animation.
        let mut second = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        let opening: zeron_doc::TranscriptUpdate =
            serde_json::from_value(second.next().await.unwrap()).unwrap();
        assert_eq!(
            opening.replay_baseline.unwrap().entries["reply"]["body"],
            "café histórico y nuevo".len()
        );

        let mut parts = vec![live];
        for ix in 0..2 {
            let id = format!("recovered-{ix}");
            parts.push(text(&id, "otro bloque histórico"));
            writer.sync(&parts).unwrap();
            sink.apply_replay_row(&source.export_snapshot().unwrap(), 2 + ix);
            for (stream, expected) in [
                (&mut first, "café histórico".len()),
                (&mut second, "café histórico y nuevo".len()),
            ] {
                let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
                    tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                        .await
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
                let baseline = update.replay_baseline.unwrap();
                assert_eq!(
                    baseline.entries["reply"].get("body"),
                    Some(&expected),
                    "replay must retain this watcher's opening cutoff, excluding later live bytes"
                );
                assert_eq!(
                    baseline.entries["reply"][&id],
                    "otro bloque histórico".len()
                );
                assert_eq!(baseline.entries["reply"].len(), 2 + ix as usize);
            }
        }
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn reopening_rearms_history_for_previously_live_text() {
        use crate::doc_host::{DocHost, DocHostConfig};
        use zeron_sync::chat_client::ChatDocSink;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "viewer".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("reopen").unwrap();
        let sink = crate::chat2_host::EngineChatSink::new(&handle.doc_arc(), store, "reopen")
            .with_handle(Arc::downgrade(&handle));
        let source = zeron_doc::SessionDoc::init("reopen").unwrap();
        let mut writer = zeron_doc::SegmentWriter::begin(&source, "reply", "host", 0).unwrap();
        let part = |text: &str| zeron_doc::MessagePart::Text {
            id: "body".into(),
            text: text.into(),
        };
        let mut stream = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        stream.next().await.unwrap();
        writer.sync(&[part("live")]).unwrap();
        sink.apply_row(&source.export_snapshot().unwrap(), 1);
        let _: serde_json::Value =
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap();
        drop(stream);
        // No unwatched commit clears provenance before the new attach.
        let mut stream = doc_messages_stream(handle.watch_messages(), handle.doc_arc());
        stream.next().await.unwrap();
        writer.sync(&[part("live plus recovered")]).unwrap();
        sink.apply_replay_row(&source.export_snapshot().unwrap(), 2);
        let update: zeron_doc::TranscriptUpdate = serde_json::from_value(
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            update.replay_baseline.unwrap().entries["reply"]["body"],
            "live plus recovered".len()
        );
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn context_only_commits_reach_remote_watch_and_reconnect() {
        let host = zeron_doc::SessionDoc::init("context-chat").unwrap();
        host.update_context_usage(Some(42000), Some(200000))
            .unwrap();
        // The viewing engine reads a replicated document, with no harness process.
        let remote = Arc::new(zeron_doc::SessionDoc::from_doc(loro::LoroDoc::new()));
        remote
            .doc()
            .import(&host.export_snapshot().unwrap())
            .unwrap();
        let (tx, rx) = watch::channel(crate::doc_host::TranscriptSnapshot::default());
        let mut stream = doc_messages_stream(rx, remote.clone());
        let first = stream.next().await.unwrap();
        assert_eq!(first["contextUsage"]["tokens"], 42000);
        assert!(first.get("reset").is_some());
        let version = host.doc().oplog_vv();
        host.update_context_usage(Some(0), None).unwrap();
        remote
            .doc()
            .import(
                &host
                    .doc()
                    .export(loro::ExportMode::updates(&version))
                    .unwrap(),
            )
            .unwrap();
        tx.send_replace(crate::doc_host::TranscriptSnapshot::default());
        let update = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(update["contextUsage"]["tokens"], 0);
        assert_eq!(update["contextUsage"]["window"], 200000);
        let mut reconnect = doc_messages_stream(tx.subscribe(), remote.clone());
        assert_eq!(
            reconnect.next().await.unwrap()["contextUsage"],
            update["contextUsage"]
        );
        remote.clear_context_usage().unwrap();
        tx.send_replace(crate::doc_host::TranscriptSnapshot::default());
        assert!(stream.next().await.unwrap()["contextUsage"].is_null());
    }
}
