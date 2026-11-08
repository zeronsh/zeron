//! zeron-rpc — the typed control plane (UiRpc / ControlRpc) over WebSocket + in-memory
//! transports, plus the device-room relay transport ({s,k,to,from} frames — [`device_room`]).
//!
//! Framing: ndjson envelopes, one JSON object per WebSocket text message (or per line on
//! byte transports), matching the shape of zeron's Effect RPC without the Effect runtime:
//!
//! - client → server: `{id, method, params}` to invoke, `{id, cancel: true}` to stop a stream;
//! - server → client: `{id, ok}` / `{id, err}` for unary calls,
//!   `{id, item}`* then `{id, done: true}` (or `{id, err}`) for streams.
//!
//! The server dispatches into an [`RpcService`]; the [`RpcClient`] offers `call` and
//! `subscribe`. Both ends run over any pair of string channels, so the in-memory transport
//! ([`memory_client`]) exercises the exact same code path as the WebSocket one.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

mod caller;
mod client;
pub mod device_room;
pub mod ipc_auth;
mod server;
mod ws;

pub use caller::{Caller, caller, with_caller};
pub use client::{RpcClient, RpcSubscription, connect_ipc, connect_ipc_as_agent, connect_ws};
pub use device_room::{
    CallerProof, CallerProofFn, DeviceFrameHeader, DeviceLink, HostRelay, HostRelayConfig,
    LinkCache, LinkCacheConfig, caller_proof_message,
    NudgeHandler, PeerLiveness, PeerLivenessProbe, StaticToken, TokenError, TokenSource,
    decode_device_frame, device_room_ws_url, encode_device_frame, is_liveness_probe,
};
pub use server::{serve_connection, serve_ws_listener, serve_ws_listener_scoped};

/// RPC method names — single source of truth for both ends.
/// Full surface: docs/research/feature-inventory.md §2.
pub mod methods {
    pub const VOICE_CAPABILITIES_V2: &str = "VoiceCapabilitiesV2";
    pub const PREPARE_VOICE_V2: &str = "PrepareVoiceV2";
    pub const OWN_VOICE_V2: &str = "OwnVoiceV2";
    pub const NEGOTIATE_VOICE_V2: &str = "NegotiateVoiceV2";
    pub const CONFIRM_VOICE_MEDIA_V2: &str = "ConfirmVoiceMediaV2";
    pub const REPORT_VOICE_MEDIA_V2: &str = "ReportVoiceMediaV2";
    pub const STOP_VOICE_V2: &str = "StopVoiceV2";
    pub const CANCEL_VOICE_ATTEMPT_V2: &str = "CancelVoiceAttemptV2";
    pub const WATCH_PREVIEWS: &str = "WatchPreviews";
    pub const LIST_HARNESSES: &str = "ListHarnesses";
    pub const CANCEL_INSTALL: &str = "CancelInstall";
    pub const INSTALL_HARNESS: &str = "InstallHarness";
    /// The ACP registry's agents as the target device sees them (`{refresh?}`
    /// → `{agents, fetchedAt, offline, platform}`), and adding (or
    /// updating) and removing one there (`{id}`).
    pub const LIST_ACP_REGISTRY: &str = "ListAcpRegistry";
    pub const ADD_ACP_AGENT: &str = "AddAcpAgent";
    pub const REMOVE_ACP_AGENT: &str = "RemoveAcpAgent";
    /// An added agent's own environment on the target device (`{id}` →
    /// `{env}`, `KEY=VALUE` a line; set with `{id, env}`): kept there,
    /// owner-only, never synced.
    pub const GET_ACP_AGENT_ENV: &str = "GetAcpAgentEnv";
    pub const SET_ACP_AGENT_ENV: &str = "SetAcpAgentEnv";
    /// An added agent's sign-in on the target device, asked of it without
    /// signing in (`{id, refresh?}` → `{signedIn, account?, canSignOut,
    /// methods}`), and signing it out there (`{id}`, ACP's `logout`).
    pub const GET_ACP_AGENT_SIGN_IN: &str = "GetAcpAgentSignIn";
    pub const SIGN_OUT_ACP_AGENT: &str = "SignOutAcpAgent";
    /// One variable of an added agent's own environment on the target device
    /// (`{id, var, value}`; an empty value forgets it): the key a sign-in
    /// method reads from there. Replies `{ok}`; the value never comes back.
    pub const SET_ACP_AGENT_KEY: &str = "SetAcpAgentKey";
    /// An agent of your own on the target device (`{name, command, env}`),
    /// run through the same generic ACP harness as the registry's.
    pub const ADD_OWN_AGENT: &str = "AddOwnAgent";
    /// Flip a harness's enablement on the target device (Settings → Providers);
    /// replies with the device's fresh `ListHarnesses` catalog.
    pub const GET_TITLE_SETTINGS: &str = "GetTitleSettings";
    pub const SET_TITLE_SETTINGS: &str = "SetTitleSettings";
    /// Whether agents' commits on this device credit Zeron
    /// (`{enabled, cloud}`; set with `{enabled}`).
    pub const GET_COMMIT_CREDIT: &str = "GetCommitCredit";
    pub const SET_COMMIT_CREDIT: &str = "SetCommitCredit";
    /// Which providers on this device run without their own subagents
    /// (`{switchable, off}`; set with `{harness, enabled}`).
    pub const GET_NATIVE_SUBAGENTS: &str = "GetNativeSubagents";
    pub const SET_NATIVE_SUBAGENTS: &str = "SetNativeSubagents";
    pub const SET_HARNESS_ENABLED: &str = "SetHarnessEnabled";
    pub const LIST_MODELS: &str = "ListModels";
    pub const LIST_SKILLS: &str = "ListSkills";
    pub const LIST_COMMANDS: &str = "ListCommands";
    pub const QUEUE_COMMAND: &str = "QueueCommand";
    pub const TAKE_PROJECT_ACTION_SETUP: &str = "TakeProjectActionSetup";
    /// Peer-to-peer delivery fallback: the SENDER's engine forwards a queued
    /// command entry (client-minted id and all) straight over the device-room
    /// link when its chat2 rows can't reach the edge but the host's peer link
    /// is alive. The host claims the id in its processed ledger before
    /// executing, so the doc row arriving later dedupes to a no-op —
    /// exactly-once by construction. Params `{chatId, entry}`.
    pub const RELAY_COMMAND: &str = "RelayCommand";
    /// User-driven delivery retry for a chat with unadopted queued sends:
    /// fresh chat2 socket, host nudge, drain pass, and a new delivery escort
    /// per pending command. Params `{chatId}`; IPC-only.
    pub const RETRY_DELIVERY: &str = "RetryDelivery";
    pub const FORK_SIDE_CHAT: &str = "ForkSideChat";
    pub const WATCH_DOC_MESSAGES: &str = "WatchDocMessages";
    /// Explicit user navigation, `{chatId}`. Prioritizes this device's sync
    /// connection; automatic subscriptions and retries must not call it.
    pub const FOCUS_CHAT: &str = "FocusChat";
    /// `{viewportId, focused}`: an app window gained or lost focus (desktop
    /// banners hold back while one is focused). Focused viewports repeat it;
    /// a report goes stale after 45 s.
    pub const SET_VIEWPORT_FOCUS: &str = "SetViewportFocus";
    /// Messages typed while the agent was busy, held on the chat doc so every
    /// device sees the same queue. `{ chatId }` → `{ items: QueuedMessage[] }`.
    pub const WATCH_QUEUE: &str = "WatchQueue";
    /// Append to the queue. `{ chatId, text, attachments?, holdForTurnEnd? }` → `{ id }`.
    pub const QUEUE_MESSAGE: &str = "QueueMessage";
    /// Retype a queued message; empty text deletes it.
    /// `{ chatId, id, text }` → `{ changed }`.
    pub const UPDATE_QUEUED_MESSAGE: &str = "UpdateQueuedMessage";
    /// Acquire a host-authoritative edit lease for one queued row.
    pub const BEGIN_QUEUED_MESSAGE_EDIT: &str = "BeginQueuedMessageEdit";
    /// Renew an acquired queue edit lease.
    pub const RENEW_QUEUED_MESSAGE_EDIT: &str = "RenewQueuedMessageEdit";
    /// Commit, cancel, discard, or explicitly release an acquired edit.
    pub const FINISH_QUEUED_MESSAGE_EDIT: &str = "FinishQueuedMessageEdit";
    /// Reorder. `{ chatId, id, toIndex }` → `{ changed }`.
    pub const MOVE_QUEUED_MESSAGE: &str = "MoveQueuedMessage";
    pub const REMOVE_QUEUED_MESSAGE: &str = "RemoveQueuedMessage";
    /// Interrupt whatever is running and send this one. `{ chatId, id }` → `{ sent }`.
    pub const SEND_QUEUED_MESSAGE_NOW: &str = "SendQueuedMessageNow";
    /// Steer this row into the live turn without interrupting it.
    /// `{ chatId, id }` → `{ sent }`.
    pub const STEER_QUEUED_MESSAGE_NOW: &str = "SteerQueuedMessageNow";
    /// Nudge every open room client to verify liveness NOW (window focus,
    /// app foregrounded). No params; IPC-only. Each room ignores the hint
    /// unless it has been broadcast-quiet ≥30s, so this is cheap to spam.
    pub const PROBE_SYNC: &str = "ProbeSync";
    /// Live sync introspection (`zeron sync` / debug surfaces): per-room
    /// connection state, last pushed-frame/ack ages, rejoin/probe/resync
    /// counters for the workspace room and every open chat doc. No params;
    /// IPC-only.
    pub const SYNC_STATUS: &str = "SyncStatus";
    /// Pushed edge-connectivity posture (`zeron_proto::Connectivity`):
    /// current value first, then every change — the connection pill /
    /// composer-honesty / queued-badge feed. No params; IPC-only.
    pub const WATCH_CONNECTIVITY: &str = "WatchConnectivity";
    /// In-flight queued-attachment transfers (`zeron_proto::TransferProgress`
    /// list): current set first, then a fresh snapshot per landed chunk —
    /// the sending thumbnail's percent-ring feed. No params; IPC-only.
    pub const WATCH_TRANSFERS: &str = "WatchTransfers";
    pub const WATCH_CHATS: &str = "WatchChats";
    pub const WATCH_SIDEBAR_PREFERENCES: &str = "WatchSidebarPreferences";
    pub const WATCH_DEVICES: &str = "WatchDevices";
    pub const WATCH_SESSIONS: &str = "WatchSessions";
    /// Spaces registry (device+folder pairs) from the workspace doc.
    pub const WATCH_SPACES: &str = "WatchSpaces";
    /// Entity mutations against the workspace doc (feature-inventory §2 DataRpc).
    /// Params are tagged `{op: createChat|createSpace|renameSpace|setSpaceIcon|
    /// deleteSpace|renameChat|setChatArchived|deleteChat|renameDevice|markChatSeen, …}`.
    pub const MUTATE: &str = "Mutate";
    /// This engine's identity → `{deviceId}` (IPC-only; never relay-forwarded —
    /// the answer is about whichever engine you are directly connected to).
    pub const LOCAL_DEVICE: &str = "LocalDevice";
    /// This engine runtime's fixed device and workspace identity.
    pub const ENGINE_INFO: &str = "EngineInfo";
    /// Readiness barrier for the engine runtime. The call completes once stores
    /// and journals are assembled, or fails with the assembly error.
    pub const ENGINE_READY: &str = "EngineReady";
    /// Ask a headless IPC owner to drain its runtime and exit successfully.
    /// Headed IPC owners do not implement this method: closing another app's
    /// engine behind its windows would leave that process unusable.
    pub const STOP_ENGINE: &str = "StopEngine";
    /// Headless IPC only: stop starting work ahead of a Cloud sleep (the
    /// edge runs `zeron quiesce` before stopping the machine). Replies
    /// `{"quiesced":true}` or `{"quiesced":false,"busy":"<why>"}`.
    pub const QUIESCE_FOR_SLEEP: &str = "QuiesceForSleep";
    pub const AUTH_STATUS: &str = "AuthStatus";
    // AuthRpc mutations (feature-inventory §2 AuthRpc; IPC-only).
    pub const SIGN_IN: &str = "SignIn";
    pub const SIGN_IN_HEADLESS: &str = "SignInHeadless";
    pub const COMPLETE_SIGN_IN: &str = "CompleteSignIn";
    pub const SIGN_OUT: &str = "SignOut";
    /// The signed-in session's current bearer, for a local client that calls
    /// the edge as the user itself (zgx's account calls): `{}` →
    /// `{token, expiresAt?}` (unix seconds). Refreshed as the engine's own
    /// calls refresh; the refresh token never leaves the engine. Local IPC
    /// only ([`crate::local_only`]): the device-room relay refuses it.
    pub const ACCESS_TOKEN: &str = "AccessToken";
    /// `{}` → `{url}`: a fresh interactive sign-in (as `SignIn`) whose
    /// exchange leaves the engine a one-time step-up ticket, kept in memory
    /// and never handed out. Needed before `DeleteAccount`.
    pub const STEP_UP: &str = "StepUp";
    /// `{confirm}` (the account's email) → `{ok: true}`, or `{ok: false,
    /// error, message, organization?}`: the engine deletes the account at
    /// the edge with its step-up ticket. `error` is `step_up_required` (run
    /// `StepUp`, then retry), `signed_out`, `offline`, `invalid`,
    /// `last_admin` (with `organization`), `busy` or `upstream`.
    pub const DELETE_ACCOUNT: &str = "DeleteAccount";
    pub const LIST_ORGS: &str = "ListOrgs";
    pub const CREATE_ORG: &str = "CreateOrg";
    pub const SELECT_ORG: &str = "SelectOrg";
    /// One-time local→synced profile import: what's importable (unary).
    pub const LOCAL_IMPORT_STATUS: &str = "LocalImportStatus";
    /// One-time local→synced profile import: run it (stream of progress items).
    pub const IMPORT_LOCAL_WORKSPACE: &str = "ImportLocalWorkspace";
    // Repos / worktrees / folders (ControlRpc, relay-forwardable).
    pub const LIST_REPOS: &str = "ListRepos";
    pub const ADD_REPO: &str = "AddRepo";
    pub const CLONE_REPO: &str = "CloneRepo";
    pub const CREATE_REPO: &str = "CreateRepo";
    pub const LIST_BRANCHES: &str = "ListBranches";
    pub const LIST_REFS: &str = "ListRefs";
    pub const LIST_GIT_HISTORY: &str = "ListGitHistory";
    /// Fuzzy commit-subject / SHA search over the complete public history.
    pub const SEARCH_GIT_HISTORY: &str = "SearchGitHistory";
    /// Resolve hosted profile images for a page of Git commit authors.
    pub const RESOLVE_GIT_AVATARS: &str = "ResolveGitAvatars";
    /// Update remote-tracking refs without changing HEAD, the index, or files.
    pub const FETCH_ALL: &str = "FetchAll";
    pub const SWITCH_REF: &str = "SwitchRef";
    pub const LIST_FOLDERS: &str = "ListFolders";
    pub const GET_WORKTREE_SETTINGS: &str = "GetWorktreeSettings";
    pub const SET_WORKTREE_SETTINGS: &str = "SetWorktreeSettings";
    /// The device's browse roots: home plus mounted drives/volumes.
    pub const LIST_DRIVES: &str = "ListDrives";
    /// Folders under the host's home by name (new-project picker).
    pub const SEARCH_HOME_FOLDERS: &str = "SearchHomeFolders";
    /// The new-project picker closed: drop the host's home index if unpinned.
    pub const RELEASE_HOME_FOLDER_SEARCH: &str = "ReleaseHomeFolderSearch";
    /// Fuzzy relative-path search rooted in a known chat or space checkout.
    pub const SEARCH_FILES: &str = "SearchFiles";
    // Device-local workspace filesystem operations. All are relay-forwardable;
    // WatchWorkspaceFiles is the only streaming method in this group.
    pub const LIST_WORKSPACE_DIRECTORY: &str = "ListWorkspaceDirectory";
    pub const SEARCH_WORKSPACE_FILES: &str = "SearchWorkspaceFiles";
    /// Start or keep alive a workspace's search index (fire-and-forget warm).
    pub const WARM_WORKSPACE_SEARCH: &str = "WarmWorkspaceSearch";
    /// Plain-text search over a workspace's file contents.
    pub const SEARCH_WORKSPACE_CONTENT: &str = "SearchWorkspaceContent";
    pub const READ_WORKSPACE_IMAGE: &str = "ReadWorkspaceImage";
    pub const READ_WORKSPACE_FILE: &str = "ReadWorkspaceFile";
    pub const MOVE_WORKSPACE_ENTRY: &str = "MoveWorkspaceEntry";
    pub const DELETE_WORKSPACE_ENTRY: &str = "DeleteWorkspaceEntry";
    /// An empty file or folder at a new path (never replacing one).
    pub const CREATE_WORKSPACE_ENTRY: &str = "CreateWorkspaceEntry";
    pub const WRITE_WORKSPACE_FILE: &str = "WriteWorkspaceFile";
    pub const WATCH_WORKSPACE_FILES: &str = "WatchWorkspaceFiles";
    pub const CREATE_WORKTREE: &str = "CreateWorktree";
    pub const DELETE_WORKTREE: &str = "DeleteWorktree";
    // Project Actions are private state on the device that owns the project.
    pub const LIST_PROJECT_ACTIONS: &str = "ListProjectActions";
    pub const UPSERT_PROJECT_ACTION: &str = "UpsertProjectAction";
    pub const DELETE_PROJECT_ACTION: &str = "DeleteProjectAction";
    pub const RUN_PROJECT_ACTION: &str = "RunProjectAction";
    // A chat's background processes (`Session::background_tasks`) live in
    // its host's runtime: `{chatId, taskId}`, relay-forwardable.
    pub const STOP_BACKGROUND_TASK: &str = "StopBackgroundTask";
    pub const READ_BACKGROUND_TASK_OUTPUT: &str = "ReadBackgroundTaskOutput";
    /// `{chatId, taskIds, waiting?}` → `{matched, unmatched}`: the agent says
    /// it waits on these of its background processes (or, `waiting` false,
    /// no longer), so its turn's end reads as a wait (Zeron's MCP
    /// `wait_on_background`). An agent's always names its own chat. Refused
    /// where the runtime doesn't wake its agent when the work ends. Local:
    /// an agent's MCP server dials its chat's host.
    pub const AWAIT_BACKGROUND: &str = "AwaitBackground";
    /// `{chatId}` → `{stopped}`: the person's Stop waiting: the processes the
    /// agent said it waits on no longer hold the chat (its background agents
    /// still do). Relay-forwardable, to the chat's host.
    pub const STOP_WAITING: &str = "StopWaiting";
    /// `{chatId, toolId, offset}` → `ToolOutputChunk`: one command's whole
    /// output, from the run journal of the device that ran it, redacted;
    /// relay-forwardable (target the chat's host).
    pub const READ_TOOL_OUTPUT: &str = "ReadToolOutput";
    // Terminals (ControlRpc, relay-forwardable; SubscribeTerminal streams).
    pub const OPEN_TERMINAL: &str = "OpenTerminal";
    pub const SUBSCRIBE_TERMINAL: &str = "SubscribeTerminal";
    pub const WRITE_TERMINAL: &str = "WriteTerminal";
    pub const RESIZE_TERMINAL: &str = "ResizeTerminal";
    pub const CLOSE_TERMINAL: &str = "CloseTerminal";
    /// Whether closing a terminal would end a program running in it.
    pub const TERMINAL_STATUS: &str = "TerminalStatus";
    /// Checkout-diff stream for the target device's chats (DataRpc,
    /// relay-forwardable — diffs are produced where the checkout lives).
    pub const WATCH_CHECKOUT_DIFFS: &str = "WatchCheckoutDiffs";
    pub const WATCH_WORKSPACE_GIT_STATUS: &str = "WatchWorkspaceGitStatus";
    /// Current pull request for one checkout, resolved on the checkout's host device.
    pub const WATCH_CHECKOUT_CHANGE_REQUEST: &str = "WatchCheckoutChangeRequest";
    /// One page of a repository's open pull requests (`filter`, `after`
    /// cursor), with the total count.
    pub const LIST_CHANGE_REQUEST_PAGE: &str = "ListChangeRequestPage";
    /// Resolve a selected checkout through local Git metadata only.
    pub const GET_CHANGE_REQUEST_REPOSITORY: &str = "GetChangeRequestRepository";
    pub const GET_CHANGE_REQUEST: &str = "GetChangeRequest";
    pub const POST_CHANGE_REQUEST_COMMENT: &str = "PostChangeRequestComment";
    /// `{url, path, line, side: "LEFT" | "RIGHT", commitId, body}` → the
    /// comment: a new review thread on one line of the changes.
    pub const POST_CHANGE_REQUEST_REVIEW_COMMENT: &str = "PostChangeRequestReviewComment";
    /// `{url, thread, body}` → the comment: a reply in a review thread.
    pub const REPLY_CHANGE_REQUEST_THREAD: &str = "ReplyChangeRequestThread";
    /// `{url, thread, resolved}` → `{resolved}`: a review thread resolved or
    /// opened again.
    pub const RESOLVE_CHANGE_REQUEST_THREAD: &str = "ResolveChangeRequestThread";
    pub const GET_CHANGE_REQUEST_DIFF: &str = "GetChangeRequestDiff";
    pub const GET_CHECKOUT_DIFF: &str = "GetCheckoutDiff";
    /// Permanently restore one chat-owned checkout to its current HEAD and
    /// remove only its untracked, non-ignored paths.
    pub const DISCARD_WORKING_TREE: &str = "DiscardWorkingTree";
    pub const GET_CHECKOUT_FILE_DIFF_TEXT: &str = "GetCheckoutFileDiffText";
    // Agent accounts (ControlRpc, relay-forwardable — CLI logins are per-device).
    pub const LIST_AGENT_ACCOUNTS: &str = "ListAgentAccounts";
    pub const ACTIVATE_AGENT_ACCOUNT: &str = "ActivateAgentAccount";
    pub const FORGET_AGENT_ACCOUNT: &str = "ForgetAgentAccount";
    pub const START_AGENT_LOGIN: &str = "StartAgentLogin";
    /// `{loginId, requesterDeviceId, path}`: replay a sign-in's browser
    /// callback the requesting device caught on its own loopback (a phone
    /// signing a computer's agent in). Answers `{status, location?}`.
    pub const RELAY_AGENT_LOGIN_CALLBACK: &str = "RelayAgentLoginCallback";
    pub const COMPLETE_AGENT_LOGIN: &str = "CompleteAgentLogin";
    pub const POLL_AGENT_LOGIN: &str = "PollAgentLogin";
    pub const CANCEL_AGENT_LOGIN: &str = "CancelAgentLogin";
    pub const CONSUME_CODEX_RESET_CREDIT: &str = "ConsumeCodexResetCredit";
    // Uploads / attachments (ControlRpc, relay-forwardable — target the chat's host device).
    pub const UPLOAD_CHUNK: &str = "UploadChunk";
    pub const UPLOAD_COMMIT: &str = "UploadCommit";
    pub const READ_ATTACHMENT_CHUNK: &str = "ReadAttachmentChunk";
    /// One chunk of an image an agent's reply names in Markdown
    /// (`![alt](path)`), read only from inside that chat's working folder.
    pub const READ_CHAT_IMAGE_CHUNK: &str = "ReadChatImageChunk";
    /// Store an agent's HTML page and show it inline in that chat's live
    /// turn (`show_visual`). Local only: the calling agent runs on this device.
    pub const PUBLISH_VISUAL: &str = "PublishVisual";
    /// Store an image file an agent shows and fold it into that chat's live
    /// turn as an image part (`show_image`). Local only, like `PublishVisual`.
    pub const PUBLISH_IMAGE: &str = "PublishImage";
    /// Lazy full-tool-output fetch from the R2 sidecar by doc-resident ref
    /// (chat2-sync A3). Edge-direct from any device — never relay-forwarded.
    pub const FETCH_TOOL_BLOB: &str = "FetchToolBlob";
    // Updates (ControlRpc, relay-forwardable — a device reports/applies its own
    // binary's update). Stream: current UpdateStatus, then every change.
    pub const UPDATE_STATUS: &str = "UpdateStatus";
    /// Download + apply the newest release on the target device (symlink-managed
    /// installs; the service restart is scheduled after the reply flushes).
    pub const APPLY_UPDATE: &str = "ApplyUpdate";
    /// Device-local agent CLI update lifecycle. The stream emits the complete
    /// ordered status list initially and after every transition.
    pub const WATCH_HARNESS_UPDATES: &str = "WatchHarnessUpdates";
    pub const CHECK_HARNESS_UPDATES: &str = "CheckHarnessUpdates";
    pub const APPLY_HARNESS_UPDATE: &str = "ApplyHarnessUpdate";
    pub const CANCEL_HARNESS_UPDATE: &str = "CancelHarnessUpdate";
    pub const DISMISS_HARNESS_UPDATE: &str = "DismissHarnessUpdate";
    pub const SET_HARNESS_UPDATE_POLICY: &str = "SetHarnessUpdatePolicy";
    // Cloud (docs/design/cloud-device.md). IPC-only: the engine calls the
    // edge's CloudAccount DO with the user's bearer; never routed by
    // targetDeviceId.
    /// The account: `{}` → `CloudStatus`.
    pub const CLOUD_STATUS: &str = "CloudStatus";
    /// Turn Cloud on (instant: mints the logical Cloud device; sandboxes are
    /// per session). `{}` → `CloudStatus`.
    pub const CLOUD_ENABLE: &str = "CloudEnable";
    /// Turn Cloud off: deletes every session sandbox (final usage captured
    /// first), revokes their vault enrollments, and removes the logical
    /// device. `{}` → `CloudStatus` (`Deleting`, then `Off`).
    pub const CLOUD_DELETE: &str = "CloudDelete";
    /// `{}` → `CloudSessions`.
    pub const CLOUD_SESSIONS: &str = "CloudSessions";
    /// `{chatId}` → `CloudSession`. Wake/sleep one session's sandbox; sending
    /// to a sleeping session wakes it on its own (the host nudge).
    pub const CLOUD_SESSION_WAKE: &str = "CloudSessionWake";
    pub const CLOUD_SESSION_SLEEP: &str = "CloudSessionSleep";
    /// `{chatId}` → `CloudSession` (`Deleting`). Deletes the sandbox and
    /// anything uncommitted in it; the transcript stays.
    pub const CLOUD_SESSION_DELETE: &str = "CloudSessionDelete";
    /// The signed-in user's Cloud credits — balance, and use per UTC day
    /// (reconciled against the sandbox provider's own meter):
    /// `{from?: "YYYY-MM-DD", to?: "YYYY-MM-DD"}` (default: the last 30 days)
    /// → `CloudUsage`.
    pub const CLOUD_USAGE: &str = "CloudUsage";
    /// The signed-in user's plan and credits, and what can be bought:
    /// `{}` → `BillingStatus`.
    pub const BILLING_STATUS: &str = "BillingStatus";
    /// A Stripe Checkout page: `{kind: "subscription", plan}` or
    /// `{kind: "credits", packs}` → `BillingLink`.
    pub const BILLING_CHECKOUT: &str = "BillingCheckout";
    /// The Stripe Billing Portal (change plan, payment method, invoices,
    /// cancel): `{}` → `BillingLink`.
    pub const BILLING_PORTAL: &str = "BillingPortal";
    // Credential vault (IPC-only; the edge forwards to the vault Worker).
    /// `{}` → `VaultStatus`.
    pub const VAULT_STATUS: &str = "VaultStatus";
    /// Sign in to Codex on THIS device (`codex login` into a throwaway
    /// CODEX_HOME, shredded afterwards) and upload the result to the vault;
    /// the device's own Codex login and account slots are untouched.
    ///
    /// Returns as soon as the sign-in page is known, in the same shape as
    /// `StartAgentLogin`: `{authorizedDevices}` →
    /// `AgentLoginStart {loginId, url, mode: "browser", callbackPort?}`.
    /// The caller opens `url` in the browser, then drives the flow with the
    /// existing account-login methods on this same engine (NO
    /// `targetDeviceId`): `PollAgentLogin {loginId}` → `AgentLoginPoll`
    /// (`pending` — may carry a late `url`; `done` once the vault holds the
    /// credential; `error` + `message`, e.g. the vault's refusal) and
    /// `CancelAgentLogin {loginId}`. After `done`, refresh with `VaultStatus`.
    /// The flow gives up after 15 minutes. Fails up front (no browser) when
    /// the account can't use Cloud.
    pub const VAULT_CONNECT_CODEX: &str = "VaultConnectCodex";
    /// Sign in to Claude on THIS device through Claude's own OAuth flow
    /// (PKCE, loopback callback) in capture-only mode — the `claudeAiOauth`
    /// tokens go straight to the vault, never into this device's live Claude
    /// login, Keychain or an account slot, so the vault stays the only
    /// refresher: `{authorizedDevices}` → `AgentLoginStart`, driven exactly
    /// like `VaultConnectCodex` (`PollAgentLogin` / `CancelAgentLogin`). When
    /// no loopback port can be bound the start comes back with
    /// `mode: "paste-code"`: finish it with `CompleteAgentLogin {loginId,
    /// code}` (its reply is this device's unchanged `AgentAccountsSnapshot`;
    /// the upload has happened once it returns), then `VaultStatus`.
    pub const VAULT_CONNECT_CLAUDE: &str = "VaultConnectClaude";
    /// `{provider: "anthropic-key"|"openai-key", key, baseUrl?,
    /// authorizedDevices}` → `VaultStatus`. `baseUrl` (Anthropic keys only):
    /// the LLM gateway or proxy Claude Code sends requests to with this key.
    pub const VAULT_PUT_API_KEY: &str = "VaultPutApiKey";
    /// `{provider, authorizedDevices}` → `VaultStatus`.
    pub const VAULT_AUTHORIZE: &str = "VaultAuthorize";
    /// `{provider}` → `VaultStatus`.
    pub const VAULT_DISCONNECT: &str = "VaultDisconnect";
    /// `{deviceId}` → `VaultStatus`.
    pub const VAULT_REVOKE_DEVICE: &str = "VaultRevokeDevice";
    /// `{authorizedDevices}` → `GithubDeviceFlow`.
    pub const GITHUB_CONNECT_START: &str = "GithubConnectStart";
    /// `{flowId}` → `GithubConnectProgress`.
    pub const GITHUB_CONNECT_POLL: &str = "GithubConnectPoll";
    /// Repositories the account's vault-held GitHub connection can see,
    /// newest push first: `{query?}` → `{repos: GithubRepo[]}`. Answered via
    /// the edge (the vault calls GitHub; the token never leaves it), so any
    /// signed-in device can list them; a Cloud session device answers with
    /// its own grant. Fails with a `github_not_connected:`-prefixed message
    /// when GitHub isn't connected.
    pub const LIST_GITHUB_REPOS: &str = "ListGithubRepos";
    /// `{repo, defaultBranch?}` → `RepoRef[]`: the branches a Cloud session
    /// of `repo` (`owner/name`) can start from, through the user's GitHub
    /// connection (the vault calls GitHub), the default first.
    pub const LIST_CLOUD_BRANCHES: &str = "ListCloudBranches";

    // ── Plugins (the plugins contract; local IPC only but PluginDeliver) ──
    /// `{}` → `{dirs: {plugins, templates}, plugins: PluginItem[]}`, by name.
    pub const LIST_PLUGINS: &str = "ListPlugins";
    /// `{}` → stream of `ListPlugins`' reply: the current list at once, then
    /// each change.
    pub const WATCH_PLUGINS: &str = "WatchPlugins";
    /// `{id, allow, hash, review?}` → `PluginItem`. Allowing needs `review`,
    /// minted by [`REVIEW_PLUGIN`] for the same `id` and `hash`: only Zeron's
    /// consent window holds one. Declining (`allow: false`) needs none.
    /// Allowing is this device's alone: never over the relay.
    pub const DECIDE_PLUGIN: &str = "DecidePlugin";
    /// `{id, hash}` → `{review}`: a one-time capability for one decision
    /// about plugin `id` as shown (`hash`) — `DecidePlugin {allow: true}` or
    /// `SignPlugin {id}` — good for a few minutes. Minted for the consent
    /// window's own host only: full-token local IPC, never an agent, never
    /// another device.
    pub const REVIEW_PLUGIN: &str = "ReviewPlugin";
    /// `{id, off}` → `PluginItem` (an allowed plugin only).
    pub const SET_PLUGIN_OFF: &str = "SetPluginOff";
    /// `{id}` → `{}`: the folder moves to `plugins/.removed/`.
    pub const REMOVE_PLUGIN: &str = "RemovePlugin";
    /// `{id, name, description?, permissions?}` → `PluginItem`.
    pub const NEW_PLUGIN: &str = "NewPlugin";
    /// `{id}` → `PluginItem`, or `{path}` → `VerifyPlugin`'s reply; both
    /// with `files` (how many files the signature covers).
    pub const SIGN_PLUGIN: &str = "SignPlugin";
    /// `{path}` → `{author, status, hash, error?}`.
    pub const VERIFY_PLUGIN: &str = "VerifyPlugin";
    /// `{}` → `{device: {id, name}, tools: [..]}`.
    pub const LIST_PLUGIN_TOOLS: &str = "ListPluginTools";
    /// `{}` → stream of `ListPluginTools`' reply.
    pub const WATCH_PLUGIN_TOOLS: &str = "WatchPluginTools";
    /// `{mcp_name | plugin+tool, args, chatId?}` → `{ok: true, value}`.
    pub const CALL_PLUGIN_TOOL: &str = "CallPluginTool";
    /// `{}` → stream (the app) of `{callId, plugin, tool, args, chatId?}`;
    /// the latest subscriber wins.
    pub const SERVE_PLUGIN_TOOLS: &str = "ServePluginTools";
    /// `{callId, ok, value?, error?}` → `{}`.
    pub const PLUGIN_TOOL_RESULT: &str = "PluginToolResult";
    /// `{plugin, to: deviceId | "*", data}` → `{results: {deviceId: outcome}}`.
    pub const PLUGIN_SEND: &str = "PluginSend";
    /// Peer to peer over the device link: `{plugin, from, author_key?, data}`
    /// → `{outcome}`. The one plugin method a peer may call.
    pub const PLUGIN_DELIVER: &str = "PluginDeliver";
    /// The app (a plugin's window): `{plugin, method, args?, device?,
    /// timeout?}` → `{ok: true, value}` | `{ok: false, reason, message}`.
    /// Its service on this device, or on `device` (sent there as
    /// [`SERVE_PLUGIN_CALL`] with the plugin's version and signing key).
    /// `method: "@watch"` has `device`'s service events reach this device's
    /// windows while they're open.
    pub const CALL_PLUGIN_SERVICE: &str = "CallPluginService";
    /// Peer to peer over the device link: `{plugin, version, authorKey?,
    /// from, method, args?, timeout?}` → as `CallPluginService`. Refused
    /// with a reason unless the plugin is installed, on and allowed here,
    /// in the same version, by the same author. A peer may call it.
    pub const SERVE_PLUGIN_CALL: &str = "ServePluginCall";
    /// `{}` → stream (the app) of `{plugin, from: {id, name}, data, at}`.
    pub const WATCH_PLUGIN_CHANNEL: &str = "WatchPluginChannel";
    /// `{}` → `{key, registered}`.
    pub const PLUGIN_SIGNING_KEY: &str = "PluginSigningKey";
    /// Another of your devices says it revoked a signing key: this device
    /// looks up its own key at once (nothing in the call is trusted; it only
    /// hastens the regular check). Answered to peers; not to agents.
    pub const CHECK_PLUGIN_KEY: &str = "CheckPluginKey";
    /// Templates (whole replacements of Zeron's interface): `{}` → `{dir,
    /// active, templates: [{id, name, description, author, version,
    /// zeron_version, builtin, active, status: "ok" | "errors" | "out_of_date"
    /// | "broken", error?, dir, preview?, imported?: {key, author}}], pending:
    /// [{pending, id, name, key, author, hash, ignored, …}]}`, Zeron's own
    /// first. "out_of_date":
    /// its `zeron_version` isn't this Zeron's (a real update since it was made).
    pub const LIST_TEMPLATES: &str = "ListTemplates";
    /// `{id, name?, from?}` → `{id, dir, files: [rel], map}`: a full copy of
    /// Zeron's app (overlaid with `from`'s files) to edit.
    pub const CREATE_TEMPLATE: &str = "CreateTemplate";
    /// `{id}` → `{active}`: the template the window runs (the app switches
    /// live).
    pub const USE_TEMPLATE: &str = "UseTemplate";
    /// `{id}` → `{id, active, root_error?, file_errors: {rel: error},
    /// out_of_date: [rel], files, inherits}`.
    pub const TEMPLATE_STATUS: &str = "TemplateStatus";
    /// `{id}` → `{deleted, active}`: set aside in the templates folder's
    /// `.removed`.
    pub const DELETE_TEMPLATE: &str = "DeleteTemplate";
    /// `{id, path}` → `{id, path, files, key}`: the template as a signed
    /// package (`path`, a `.zeron-template` file), signed with this device's
    /// plugin key in the template domain. Never-templated files are left out.
    pub const EXPORT_TEMPLATE: &str = "ExportTemplate";
    /// `{path}` → yours (signed by this device's key): installed,
    /// `{installed, name, author, ignored}`; anyone else's: kept for you to
    /// allow in Zeron, `{pending, id, name, …, key, author, hash, ignored,
    /// needs_consent: true}`. Unsigned or tampered packages are refused.
    pub const IMPORT_TEMPLATE: &str = "ImportTemplate";
    /// `{pending, allow, hash}` → `{installed, name, author}` or
    /// `{declined}`: the consent window's answer for a waiting package
    /// (checked again; `hash` is the package as shown). Only Zeron's consent
    /// window calls it.
    pub const DECIDE_TEMPLATE: &str = "DecideTemplate";
    /// `{pending, hash}` → `{review}`: as [`REVIEW_PLUGIN`], for
    /// `DecideTemplate {allow: true}` on that waiting package.
    pub const REVIEW_TEMPLATE: &str = "ReviewTemplate";
    /// Each template installed from a link checked there for a newer
    /// version (`{id?}`): `{updates, errors}`.
    pub const CHECK_TEMPLATE_UPDATES: &str = "CheckTemplateUpdates";
    /// The update found for a template, installed in its place (`{id}`).
    pub const UPDATE_TEMPLATE: &str = "UpdateTemplate";
    /// `{}` → `{screens: [{module, file, title, description, elements}],
    /// map}`: each screen's named elements (`#id`, `.class`) from its source;
    /// `DescribeElements` is the live window's.
    pub const DESCRIBE_APP: &str = "DescribeApp";
    /// Themes (the window's colours, apart from its template; all data,
    /// this device's, in the data dir: `zeron_theme::catalog`): `{}` →
    /// `{themes: [{id, name, source, editable, variants: {light?, dark?},
    /// in_use, template?, problem?}], wearing: {light, dark}, chosen, guide}`.
    pub const LIST_THEMES: &str = "ListThemes";
    /// `{id}` → `{id, name, source, editable, light?, dark?, adjusted, note?}`:
    /// a theme's palettes, in the shape `CreateTheme` takes.
    pub const GET_THEME: &str = "GetTheme";
    /// `{name, light?, dark?}` (palettes) or `{name?, vscode}` (a VS Code
    /// colour theme's JSON) → `{id, name, variants, adjusted}`: a theme of
    /// yours.
    pub const CREATE_THEME: &str = "CreateTheme";
    /// `{id, name?, light?, dark?}` (a palette, or null to take that scheme
    /// away) → `{id, name, variants, adjusted}`: yours only.
    pub const UPDATE_THEME: &str = "UpdateTheme";
    /// `{id}` → `{deleted}`: yours only; a scheme it was worn for goes back.
    pub const DELETE_THEME: &str = "DeleteTheme";
    /// `{id, slot?: "light" | "dark" | "both"}` → `{theme, name, worn_for,
    /// chosen}`: what the window wears.
    pub const USE_THEME: &str = "UseTheme";
    /// `{}` → stream (the app) of `{changed: true}`, after each change the
    /// theme methods make, for the window to read the files again.
    pub const WATCH_THEMES: &str = "WatchThemes";
    /// `{}` → `{elements: [{selector, place, about, provides, present}],
    /// everywhere, how}`: where plugins can place their nodes, as the main
    /// window running now has it (it writes the map as its template
    /// changes).
    pub const DESCRIBE_ELEMENTS: &str = "DescribeElements";
    /// `{id, snapshot?}` → `{id, name, status, error?, permissions, allowed,
    /// open, on_in_window?, placements?: [{at, how, count, regions}],
    /// problem?, hidden?, failed?, pages?, snapshot?}`: where a plugin's nodes
    /// landed in the window, for an agent to check its work (and, with
    /// `snapshot`, a picture of the window cut to them).
    pub const PLUGIN_STATUS: &str = "PluginStatus";
    /// `{dir}` → the list: the app's folder of Zeron's own plugins, scanned
    /// with yours (allowed what they ask, on unless turned off, never
    /// removed).
    pub const SET_BUILTIN_PLUGINS: &str = "SetBuiltinPlugins";
    /// `{path}` (a `.zeron-plugin` file) or `{url, ref?, folder?}` (a
    /// package link, a GitHub release, a git repository) → `{plugin,
    /// update, from?, added, removed, waiting}`: someone else's plugin
    /// installed, verified first; it waits for you in the app (`waiting`)
    /// unless it's signed by your own key. Over an installed one only when
    /// the same key signed both (an update).
    pub const INSTALL_PLUGIN: &str = "InstallPlugin";
    /// `{id?}` → `{updates: [{id, name, version, added, removed,
    /// refused?}], errors: [{id, name, error}]}`: each installed plugin's
    /// source checked for a new version (listed on its item as `update`).
    pub const CHECK_PLUGIN_UPDATES: &str = "CheckPluginUpdates";
    /// `{id}` → as `InstallPlugin`: the update found for it installed
    /// (checked first when none was found yet). Refused when someone else
    /// signed it or it was changed; new permissions wait for you again.
    pub const UPDATE_PLUGIN: &str = "UpdatePlugin";
    /// `{id, to?}` → `{path}`: a plugin as a `.zeron-plugin` package (an
    /// own plugin is re-signed first), into the folder or file `to`.
    pub const EXPORT_PLUGIN: &str = "ExportPlugin";
    /// `{source, title, body, session?}` → `{outcome: "posted" | "off" |
    /// "held" | "paced" | "unavailable"}`: a banner asked for (a plugin's),
    /// shown as the engine's own are, under the same Settings, at most a few
    /// a minute for each `source`; clicking it opens `session`.
    pub const POST_NOTIFICATION: &str = "PostNotification";
}

/// Methods answered only over this device's own local IPC, never to a peer
/// device through the device-room relay ([`HostRelay`] refuses them): they
/// hand the caller this device's credentials.
pub fn local_only(method: &str) -> bool {
    matches!(
        method,
        methods::ACCESS_TOKEN
            | methods::STEP_UP
            | methods::DELETE_ACCOUNT
            | methods::REVIEW_PLUGIN
            | methods::REVIEW_TEMPLATE
            // Plugins: authoring, tools and the window's channel stay this
            // device's. Your other devices may list, review, allow, turn
            // off, install and remove plugins here (the account's own
            // devices: Settings → Plugins' device switcher), deliver a
            // message, and call a service (`ServePluginCall`).
            | methods::NEW_PLUGIN
            | methods::SIGN_PLUGIN
            | methods::VERIFY_PLUGIN
            | methods::LIST_PLUGIN_TOOLS
            | methods::WATCH_PLUGIN_TOOLS
            | methods::CALL_PLUGIN_TOOL
            | methods::SERVE_PLUGIN_TOOLS
            | methods::PLUGIN_TOOL_RESULT
            | methods::PLUGIN_SEND
            | methods::CALL_PLUGIN_SERVICE
            | methods::WATCH_PLUGIN_CHANNEL
            | methods::PLUGIN_SIGNING_KEY
            | methods::LIST_TEMPLATES
            | methods::CREATE_TEMPLATE
            | methods::USE_TEMPLATE
            | methods::TEMPLATE_STATUS
            | methods::DELETE_TEMPLATE
            | methods::EXPORT_TEMPLATE
            | methods::IMPORT_TEMPLATE
            | methods::DECIDE_TEMPLATE
            | methods::CHECK_TEMPLATE_UPDATES
            | methods::UPDATE_TEMPLATE
            | methods::DESCRIBE_APP
            | methods::DESCRIBE_ELEMENTS
            // Themes: this device's library and choice, never another's.
            | methods::LIST_THEMES
            | methods::GET_THEME
            | methods::CREATE_THEME
            | methods::UPDATE_THEME
            | methods::DELETE_THEME
            | methods::USE_THEME
            | methods::WATCH_THEMES
            | methods::SET_BUILTIN_PLUGINS
            | methods::POST_NOTIFICATION
            | methods::EXPORT_PLUGIN
    )
}

/// Methods a peer device may not have answered here even though they're
/// relay-forwardable: allowing a plugin is this device's person's decision,
/// made in this device's consent window; and a file path names a file on
/// the caller's device, never this one. Declining, turning off, removing,
/// and installing from a link (which lands waiting here) stay open to your
/// other devices (Settings → Plugins' device switcher).
pub fn peer_refused(method: &str, params: &serde_json::Value) -> bool {
    if local_only(method) {
        return true;
    }
    match method {
        methods::DECIDE_PLUGIN => params.get("allow").is_none_or(|allow| allow != &serde_json::Value::Bool(false)),
        methods::INSTALL_PLUGIN => params.get("path").is_some_and(|path| !path.is_null()),
        _ => false,
    }
}

/// The methods an agent's `zeron mcp` server may call with the
/// agent-scoped IPC token: what its tools need and nothing that grants
/// power. An allow-list, so a method added later stays the app's until
/// it's listed here. Never here: deciding or reviewing (`Decide*`,
/// `Review*`), signing (`SignPlugin`, `PluginSigningKey`), updating
/// (`UpdatePlugin`, `UpdateTemplate`, the engine's and harnesses'),
/// credentials (`AccessToken`, sign-in, `StepUp`, `DeleteAccount`),
/// terminals, files, vault and agent logins, and a command's whole output
/// (`ReadToolOutput`: an agent reads a chat as its doc keeps it).
pub fn agent_allowed(method: &str) -> bool {
    matches!(
        method,
        // Discovery.
        methods::ENGINE_INFO
            | methods::LOCAL_DEVICE
            | methods::WATCH_DEVICES
            | methods::WATCH_SPACES
            | methods::WATCH_CHATS
            | methods::WATCH_SESSIONS
            | methods::WATCH_DOC_MESSAGES
            | methods::LIST_HARNESSES
            | methods::LIST_MODELS
            | methods::LIST_AGENT_ACCOUNTS
            // Chats: create, message, steer, archive (`Mutate`).
            | methods::MUTATE
            | methods::QUEUE_MESSAGE
            | methods::QUEUE_COMMAND
            // Showing the person something in the calling chat.
            | methods::PUBLISH_VISUAL
            | methods::PUBLISH_IMAGE
            // Saying what the calling chat's agent waits on.
            | methods::AWAIT_BACKGROUND
            // Plugins: authoring, installing (lands waiting for the person),
            // checking for updates, and their tools.
            | methods::LIST_PLUGINS
            | methods::PLUGIN_STATUS
            | methods::NEW_PLUGIN
            | methods::INSTALL_PLUGIN
            | methods::CHECK_PLUGIN_UPDATES
            | methods::LIST_PLUGIN_TOOLS
            | methods::WATCH_PLUGIN_TOOLS
            | methods::CALL_PLUGIN_TOOL
            | methods::DESCRIBE_ELEMENTS
            // Templates: authoring and switching (the app shows an
            // unapproved one as a preview until the person keeps it), and
            // importing (lands waiting for the person).
            | methods::LIST_TEMPLATES
            | methods::DESCRIBE_APP
            | methods::CREATE_TEMPLATE
            | methods::USE_TEMPLATE
            | methods::TEMPLATE_STATUS
            | methods::DELETE_TEMPLATE
            | methods::EXPORT_TEMPLATE
            | methods::IMPORT_TEMPLATE
            // Themes: data only (no consent: a theme can't run anything),
            // read, made, changed, deleted (yours only) and worn. Not the
            // window's watch.
            | methods::LIST_THEMES
            | methods::GET_THEME
            | methods::CREATE_THEME
            | methods::UPDATE_THEME
            | methods::DELETE_THEME
            | methods::USE_THEME
    )
}

/// Stable capability error codes transported without provider stderr.
pub mod capability_errors {
    pub const PULL_REQUESTS_CLI_UNAVAILABLE: &str = "pull_requests.cli_unavailable";
    pub const PULL_REQUESTS_AUTHENTICATION: &str = "pull_requests.authentication";
    pub const PULL_REQUESTS_RATE_LIMITED: &str = "pull_requests.rate_limited";
    pub const PULL_REQUESTS_TIMEOUT: &str = "pull_requests.timeout";
    pub const PULL_REQUESTS_DECODE: &str = "pull_requests.decode";
    pub const PULL_REQUESTS_COMMAND_FAILED: &str = "pull_requests.command_failed";
    /// `ReadToolOutput`: the host's journal doesn't hold that command (an
    /// old chat, or one whose journal is gone).
    pub const TOOL_OUTPUT_UNAVAILABLE: &str = "tool_output.unavailable";
    /// `ReadToolOutput`: the command has no result yet.
    pub const TOOL_OUTPUT_NOT_FINISHED: &str = "tool_output.not_finished";
}

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("unknown method: {0}")]
    UnknownMethod(String),
    #[error("bad params: {0}")]
    BadParams(String),
    /// Stable machine-readable capability failure preserved across transports.
    #[error("capability error: {0}")]
    Capability(String),
    #[error("{0}")]
    Failed(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("connection closed")]
    Closed,
}

/// A client-originated frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientFrame {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancel: bool,
}

/// A server-originated frame. Exactly one of `ok` / `err` / `item` / `done` is meaningful.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerFrame {
    pub id: u64,
    /// `"ok": null` is a reply whose value is `null` (for example an absent
    /// `Option`), distinct from a frame without `ok`.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub ok: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

fn present_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// What a service returns for one invocation.
pub enum RpcReply {
    /// Unary response — sent as `{id, ok}`.
    Value(serde_json::Value),
    /// Stream — each item sent as `{id, item}`, then `{id, done: true}` when it ends.
    Stream(BoxStream<'static, serde_json::Value>),
}

impl RpcReply {
    /// Serialize a value into a unary reply.
    pub fn value<T: Serialize>(value: &T) -> Result<Self, RpcError> {
        serde_json::to_value(value)
            .map(RpcReply::Value)
            .map_err(|e| RpcError::Failed(format!("serialize response: {e}")))
    }
}

/// Server-side dispatch: one implementation serves every transport.
#[async_trait]
pub trait RpcService: Send + Sync + 'static {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError>;
}

/// Deserialize typed params out of the envelope's `params` value.
pub fn parse_params<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::BadParams(e.to_string()))
}

/// Spawn an in-memory server for `service` and return a connected client.
/// Same envelopes, same dispatch loop as the WebSocket path — the in-process UI
/// transport (ARCHITECTURE §1 "zero serialization shortcuts").
pub fn memory_client(service: Arc<dyn RpcService>) -> RpcClient {
    let (client_out, server_in) = tokio::sync::mpsc::channel::<String>(256);
    let (server_out, client_in) = tokio::sync::mpsc::channel::<String>(256);
    tokio::spawn(serve_connection(service, server_out, server_in));
    RpcClient::new(client_out, client_in)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::Mutex;

    struct TestService;

    struct CancelAwareService {
        dropped: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(dropped) = self.0.take() {
                let _ = dropped.send(());
            }
        }
    }

    #[async_trait]
    impl RpcService for CancelAwareService {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            if !matches!(
                method,
                methods::WATCH_CHECKOUT_CHANGE_REQUEST | methods::WATCH_HARNESS_UPDATES | "Silent"
            ) {
                return Err(RpcError::UnknownMethod(method.into()));
            }
            let guard = DropSignal(self.dropped.lock().unwrap().take());
            let stream = futures::stream::unfold(guard, |guard| async move {
                let item = std::future::pending::<Option<(serde_json::Value, DropSignal)>>().await;
                drop(guard);
                item
            });
            if method == methods::WATCH_HARNESS_UPDATES {
                // Update watches send an initial status snapshot, then may stay
                // quiet indefinitely. Legacy peers do not send a readiness frame.
                return Ok(RpcReply::Stream(
                    futures::stream::once(async { serde_json::json!([]) })
                        .chain(stream)
                        .boxed(),
                ));
            }
            Ok(RpcReply::Stream(stream.boxed()))
        }
    }

    #[async_trait]
    impl RpcService for TestService {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            match method {
                "Echo" => Ok(RpcReply::Value(params)),
                "Count" => {
                    let n = params.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
                    Ok(RpcReply::Stream(
                        futures::stream::iter((0..n).map(|i| serde_json::json!(i))).boxed(),
                    ))
                }
                "Never" => Ok(RpcReply::Stream(futures::stream::pending().boxed())),
                "Boom" => Err(RpcError::Failed("boom".into())),
                "Capability" => Err(RpcError::Capability(
                    capability_errors::PULL_REQUESTS_AUTHENTICATION.into(),
                )),
                other => Err(RpcError::UnknownMethod(other.into())),
            }
        }
    }

    #[test]
    fn plugin_methods_are_local_only_but_managing_delivering_and_serving() {
        for method in [
            methods::NEW_PLUGIN,
            methods::SIGN_PLUGIN,
            methods::VERIFY_PLUGIN,
            methods::LIST_PLUGIN_TOOLS,
            methods::WATCH_PLUGIN_TOOLS,
            methods::CALL_PLUGIN_TOOL,
            methods::SERVE_PLUGIN_TOOLS,
            methods::PLUGIN_TOOL_RESULT,
            methods::PLUGIN_SEND,
            methods::CALL_PLUGIN_SERVICE,
            methods::WATCH_PLUGIN_CHANNEL,
            methods::PLUGIN_SIGNING_KEY,
            methods::LIST_TEMPLATES,
            methods::CREATE_TEMPLATE,
            methods::USE_TEMPLATE,
            methods::TEMPLATE_STATUS,
            methods::DELETE_TEMPLATE,
            methods::EXPORT_TEMPLATE,
            methods::IMPORT_TEMPLATE,
            methods::DECIDE_TEMPLATE,
            methods::CHECK_TEMPLATE_UPDATES,
            methods::UPDATE_TEMPLATE,
            methods::DESCRIBE_APP,
            methods::LIST_THEMES,
            methods::GET_THEME,
            methods::CREATE_THEME,
            methods::UPDATE_THEME,
            methods::DELETE_THEME,
            methods::USE_THEME,
            methods::WATCH_THEMES,
        ] {
            assert!(local_only(method), "{method}");
        }
        assert!(!local_only(methods::PLUGIN_DELIVER));
        assert!(!local_only(methods::SERVE_PLUGIN_CALL));
        // Your other devices manage this one's plugins (Settings → Plugins).
        for method in [
            methods::LIST_PLUGINS,
            methods::WATCH_PLUGINS,
            methods::DECIDE_PLUGIN,
            methods::SET_PLUGIN_OFF,
            methods::REMOVE_PLUGIN,
            methods::INSTALL_PLUGIN,
            methods::CHECK_PLUGIN_UPDATES,
            methods::UPDATE_PLUGIN,
            methods::PLUGIN_STATUS,
        ] {
            assert!(!local_only(method), "{method}");
        }
    }

    #[tokio::test]
    async fn memory_call_stream_and_error() {
        let client = memory_client(Arc::new(TestService));

        let echoed = client
            .call("Echo", serde_json::json!({"x": 1}))
            .await
            .unwrap();
        assert_eq!(echoed, serde_json::json!({"x": 1}));

        let mut items = client
            .subscribe("Count", serde_json::json!({"n": 3}))
            .await
            .unwrap();
        let mut seen = Vec::new();
        while let Some(v) = items.recv().await {
            seen.push(v);
        }
        assert_eq!(
            seen,
            vec![
                serde_json::json!(0),
                serde_json::json!(1),
                serde_json::json!(2)
            ]
        );

        let err = client
            .call("Boom", serde_json::Value::Null)
            .await
            .unwrap_err();
        assert!(matches!(err, RpcError::Failed(m) if m == "boom"));

        let err = client
            .call("Capability", serde_json::Value::Null)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RpcError::Capability(code)
                if code == capability_errors::PULL_REQUESTS_AUTHENTICATION
        ));
    }

    #[tokio::test]
    async fn checked_stream_acknowledges_support_and_preserves_unknown_method() {
        let client = memory_client(Arc::new(TestService));

        let mut items = client
            .subscribe_checked("Count", serde_json::json!({"n": 1}))
            .await
            .unwrap();
        assert_eq!(items.recv().await, Some(serde_json::json!(0)));
        assert_eq!(items.recv().await, None);

        let error = match client
            .subscribe_checked("FutureStream", serde_json::Value::Null)
            .await
        {
            Ok(_) => panic!("old service must reject unknown stream"),
            Err(error) => error,
        };
        assert!(matches!(error, RpcError::UnknownMethod(method) if method == "FutureStream"));
    }

    #[tokio::test]
    async fn dropping_checked_subscription_cancels_pending_server_stream() {
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let client = memory_client(Arc::new(CancelAwareService {
            dropped: Mutex::new(Some(dropped_tx)),
        }));
        let stream = client
            .subscribe_checked(
                methods::WATCH_CHECKOUT_CHANGE_REQUEST,
                serde_json::Value::Null,
            )
            .await
            .unwrap();

        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
            .await
            .expect("server stream cancelled")
            .expect("drop signal");
    }

    #[tokio::test]
    async fn scoped_subscription_returns_before_first_item_and_cancels_silence() {
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let service = Arc::new(CancelAwareService {
            dropped: Mutex::new(Some(dropped_tx)),
        });
        let client = memory_client(service.clone());
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.subscribe_scoped("Silent", serde_json::Value::Null),
        )
        .await
        .unwrap()
        .unwrap();
        while service.dropped.lock().unwrap().is_some() {
            tokio::task::yield_now().await;
        }
        drop(stream);
        tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn dropping_one_device_update_watch_preserves_the_other() {
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (second_tx, mut second_rx) = tokio::sync::oneshot::channel();
        let first = memory_client(Arc::new(CancelAwareService {
            dropped: Mutex::new(Some(first_tx)),
        }));
        let second = memory_client(Arc::new(CancelAwareService {
            dropped: Mutex::new(Some(second_tx)),
        }));
        let first_watch = first
            .subscribe_checked(methods::WATCH_HARNESS_UPDATES, serde_json::Value::Null)
            .await
            .unwrap();
        let second_watch = second
            .subscribe_checked(methods::WATCH_HARNESS_UPDATES, serde_json::Value::Null)
            .await
            .unwrap();

        drop(first_watch);
        tokio::time::timeout(std::time::Duration::from_secs(1), first_rx)
            .await
            .expect("first device's quiet stream cancelled")
            .expect("first drop signal");
        assert!(matches!(
            second_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        drop(second_watch);
        tokio::time::timeout(std::time::Duration::from_secs(1), second_rx)
            .await
            .expect("second device's quiet stream cancelled")
            .expect("second drop signal");
    }

    #[tokio::test]
    async fn unary_null_reply_resolves_the_call() {
        let client = memory_client(Arc::new(TestService));
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.call("Echo", serde_json::Value::Null),
        )
        .await
        .expect("a null reply is a reply")
        .unwrap();
        assert_eq!(reply, serde_json::Value::Null);
        let frame: ServerFrame = serde_json::from_str(r#"{"id":1,"ok":null}"#).unwrap();
        assert_eq!(frame.ok, Some(serde_json::Value::Null));
        let frame: ServerFrame = serde_json::from_str(r#"{"id":1,"done":true}"#).unwrap();
        assert_eq!(frame.ok, None);
    }

    /// Serve `TestService` on an ephemeral port behind `token`.
    async fn serve_test(token: &str) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve_ws_listener(
            listener,
            Arc::new(TestService),
            token.to_string(),
        ));
        port
    }

    #[tokio::test]
    async fn websocket_round_trip() {
        let token = ipc_auth::generate_token().unwrap();
        let port = serve_test(&token).await;

        let client = connect_ws(&format!("ws://127.0.0.1:{port}"), Some(&token))
            .await
            .unwrap();
        let echoed = client
            .call("Echo", serde_json::json!("hello"))
            .await
            .unwrap();
        assert_eq!(echoed, serde_json::json!("hello"));

        let mut items = client
            .subscribe("Count", serde_json::json!({"n": 2}))
            .await
            .unwrap();
        assert_eq!(items.recv().await, Some(serde_json::json!(0)));
        assert_eq!(items.recv().await, Some(serde_json::json!(1)));
        assert_eq!(items.recv().await, None);
    }

    /// A reply past tungstenite's default 16 MiB frame cap (a long chat's
    /// transcript watch) must arrive, not drop the connection.
    #[tokio::test]
    async fn websocket_carries_messages_past_the_default_frame_cap() {
        let token = ipc_auth::generate_token().unwrap();
        let port = serve_test(&token).await;
        // Multi-byte chars so fragment boundaries land mid-codepoint.
        let big = "é".repeat(9_500_000);

        let client = connect_ws(&format!("ws://127.0.0.1:{port}"), Some(&token))
            .await
            .unwrap();
        let echoed = client.call("Echo", serde_json::json!(big)).await.unwrap();
        assert_eq!(echoed.as_str().map(str::len), Some(big.len()));
        assert_eq!(client.close_reason(), None);

        // A peer still on tungstenite's default limits (an older app build)
        // reassembles the fragmented reply too.
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
        let mut req = format!("ws://127.0.0.1:{port}")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (mut legacy, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let request = serde_json::to_string(&ClientFrame {
            id: 1,
            method: Some("Echo".into()),
            params: serde_json::json!(big),
            cancel: false,
        })
        .unwrap();
        // It sends one unfragmented frame past 16 MiB: the raised cap takes it.
        assert!(request.len() > 16 << 20);
        legacy.send(Message::Text(request)).await.unwrap();
        let reply = loop {
            match legacy.next().await.unwrap().unwrap() {
                Message::Text(text) => break text,
                _ => continue,
            }
        };
        let frame: ServerFrame = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            frame.ok.and_then(|v| v.as_str().map(str::len)),
            Some(big.len())
        );
    }

    /// Raw handshake status for a dial with the given headers (`None` = served).
    async fn handshake_status(port: u16, headers: &[(&str, &str)]) -> Option<u16> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://127.0.0.1:{port}")
            .into_client_request()
            .unwrap();
        for (name, value) in headers {
            req.headers_mut().insert(
                tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
                    .unwrap(),
                value.parse().unwrap(),
            );
        }
        match tokio_tungstenite::connect_async(req).await {
            Ok(_) => None,
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                Some(response.status().as_u16())
            }
            Err(other) => panic!("unexpected handshake failure: {other}"),
        }
    }

    #[tokio::test]
    async fn handshake_with_origin_header_is_rejected() {
        let token = ipc_auth::generate_token().unwrap();
        let port = serve_test(&token).await;
        let bearer = format!("Bearer {token}");

        // A browser page opening ws://127.0.0.1:{port} always sends Origin;
        // the server must refuse the handshake before serving any RPC — even
        // when the bearer is right.
        assert_eq!(
            handshake_status(
                port,
                &[
                    ("origin", "https://evil.example"),
                    ("authorization", &bearer)
                ]
            )
            .await,
            Some(403),
            "handshake carrying an Origin header must be rejected"
        );

        // A native viewport (no Origin) still connects and can call RPC — the
        // reject must not be a blanket denial.
        let client = connect_ws(&format!("ws://127.0.0.1:{port}"), Some(&token))
            .await
            .unwrap();
        let echoed = client.call("Echo", serde_json::json!("ok")).await.unwrap();
        assert_eq!(echoed, serde_json::json!("ok"));
    }

    /// Answers every method with the caller it ran as.
    struct WhoAsked;
    #[async_trait]
    impl RpcService for WhoAsked {
        async fn handle(&self, _method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
            Ok(RpcReply::Value(serde_json::json!(format!("{:?}", crate::caller()))))
        }
    }

    /// R6/R16: the agent-scoped token (what `zeron mcp` holds) reaches its
    /// tools' methods as an agent, and nothing that decides, signs, updates
    /// or hands out credentials; the full token still reaches everything.
    #[tokio::test]
    async fn the_agent_token_refuses_everything_that_grants_power() {
        let tokens = ipc_auth::IpcTokens::generate().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve_ws_listener_scoped(listener, Arc::new(WhoAsked), tokens.clone()));
        let url = format!("ws://127.0.0.1:{port}");

        let agent = connect_ws(&url, Some(&tokens.agent)).await.unwrap();
        for method in [
            methods::DECIDE_PLUGIN,
            methods::REVIEW_PLUGIN,
            methods::SIGN_PLUGIN,
            methods::PLUGIN_SIGNING_KEY,
            methods::DECIDE_TEMPLATE,
            methods::REVIEW_TEMPLATE,
            methods::UPDATE_PLUGIN,
            methods::UPDATE_TEMPLATE,
            methods::APPLY_UPDATE,
            methods::ACCESS_TOKEN,
            methods::STEP_UP,
            methods::DELETE_ACCOUNT,
            methods::SIGN_IN,
            methods::COMPLETE_SIGN_IN,
            methods::SIGN_OUT,
            methods::SET_PLUGIN_OFF,
            methods::REMOVE_PLUGIN,
            methods::SET_BUILTIN_PLUGINS,
            methods::OPEN_TERMINAL,
            methods::WRITE_TERMINAL,
            methods::WRITE_WORKSPACE_FILE,
            methods::EXPORT_PLUGIN,
            // The window's own watch of the theme files.
            methods::WATCH_THEMES,
            methods::READ_TOOL_OUTPUT,
        ] {
            assert!(!agent_allowed(method), "{method}");
            match agent.call(method, serde_json::json!({})).await {
                Err(RpcError::Failed(message)) => assert!(message.contains("isn't open to agents"), "{method}: {message}"),
                other => panic!("an agent reached {method}: {other:?}"),
            }
        }
        let asked = agent.call(methods::LIST_PLUGINS, serde_json::json!({})).await.unwrap();
        assert!(asked.as_str().unwrap().starts_with("Agent"), "{asked}");
        // The theme tools' methods, exactly: data only.
        for method in [methods::LIST_THEMES, methods::GET_THEME, methods::CREATE_THEME, methods::UPDATE_THEME, methods::DELETE_THEME, methods::USE_THEME] {
            assert!(agent_allowed(method), "{method}");
            let asked = agent.call(method, serde_json::json!({})).await.unwrap();
            assert!(asked.as_str().unwrap().starts_with("Agent"), "{method}: {asked}");
        }

        let app = connect_ws(&url, Some(&tokens.full)).await.unwrap();
        for method in [methods::DECIDE_PLUGIN, methods::ACCESS_TOKEN] {
            let asked = app.call(method, serde_json::json!({})).await.unwrap();
            assert_eq!(asked, serde_json::json!("Local"), "{method}");
        }
    }

    #[test]
    fn peers_may_decline_but_never_allow() {
        let hash = "ab".repeat(32);
        assert!(peer_refused(methods::DECIDE_PLUGIN, &serde_json::json!({"id": "x", "allow": true, "hash": hash})));
        assert!(peer_refused(methods::DECIDE_PLUGIN, &serde_json::json!({"id": "x", "allow": "yes"})));
        assert!(!peer_refused(methods::DECIDE_PLUGIN, &serde_json::json!({"id": "x", "allow": false})));
        assert!(peer_refused(methods::INSTALL_PLUGIN, &serde_json::json!({"path": "/x"})));
        assert!(!peer_refused(methods::INSTALL_PLUGIN, &serde_json::json!({"url": "https://x", "path": null})));
        assert!(peer_refused(methods::REVIEW_PLUGIN, &serde_json::json!({})));
        assert!(peer_refused(methods::SIGN_PLUGIN, &serde_json::json!({})));
        assert!(peer_refused(methods::DELETE_ACCOUNT, &serde_json::json!({})));
    }

    #[tokio::test]
    async fn handshake_requires_the_engine_bearer() {
        let token = ipc_auth::generate_token().unwrap();
        let port = serve_test(&token).await;

        // Any local process can reach the port; without the token it gets 401.
        assert_eq!(handshake_status(port, &[]).await, Some(401));
        let wrong = format!("Bearer {}", ipc_auth::generate_token().unwrap());
        assert_eq!(
            handshake_status(port, &[("authorization", &wrong)]).await,
            Some(401)
        );
        assert_eq!(
            handshake_status(port, &[("authorization", &token)]).await,
            Some(401),
            "the Bearer scheme is required"
        );
        let url = format!("ws://127.0.0.1:{port}");
        assert!(connect_ws(&url, None).await.is_err());
        let error = match connect_ws(&url, Some("not-the-token")).await {
            Ok(_) => panic!("a wrong token must not be served"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("401"), "{error}");

        // The right token is served.
        let right = format!("Bearer {token}");
        assert_eq!(
            handshake_status(port, &[("authorization", &right)]).await,
            None
        );
        let client = connect_ws(&url, Some(&token)).await.unwrap();
        assert_eq!(
            client.call("Echo", serde_json::json!(7)).await.unwrap(),
            serde_json::json!(7)
        );
    }

    #[tokio::test]
    async fn connect_ipc_reads_the_published_token_at_dial_time() {
        let dir = tempfile::tempdir().unwrap();
        // Engine start #1.
        let first = ipc_auth::issue_token(dir.path()).unwrap();
        let first_port = serve_test(&first).await;
        let client = connect_ipc(first_port, Some(dir.path())).await.unwrap();
        assert_eq!(
            client.call("Echo", serde_json::json!(1)).await.unwrap(),
            serde_json::json!(1)
        );

        // Engine restart: a new token replaces the file. The next dial picks
        // it up without any client-side configuration.
        let second = ipc_auth::issue_token(dir.path()).unwrap();
        let second_port = serve_test(&second).await;
        let client = connect_ipc(second_port, Some(dir.path())).await.unwrap();
        assert_eq!(
            client.call("Echo", serde_json::json!(2)).await.unwrap(),
            serde_json::json!(2)
        );

        // A client holding only the stale file is refused with a clear reason.
        let stale = tempfile::tempdir().unwrap();
        ipc_auth::write_token(&ipc_auth::token_path(stale.path()), &first).unwrap();
        let error = match connect_ipc(second_port, Some(stale.path())).await {
            Ok(_) => panic!("a rotated token must not be served"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("rejected"), "{error}");
    }

    #[tokio::test]
    async fn dropping_stream_receiver_cancels_server_side() {
        let client = memory_client(Arc::new(TestService));
        let items = client
            .subscribe("Never", serde_json::Value::Null)
            .await
            .unwrap();
        drop(items);
        // The next unary call still works — the dead stream didn't wedge the connection.
        let echoed = client.call("Echo", serde_json::json!(2)).await.unwrap();
        assert_eq!(echoed, serde_json::json!(2));
    }
}
