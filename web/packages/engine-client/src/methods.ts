/**
 * RPC method names, mirroring `zeron_rpc::methods` (crates/rpc/src/lib.rs).
 * Wire names are frozen (ADR 0005); wiregen does not emit the method table
 * yet, so this module carries the subset the connection core needs and the
 * tests drive. Extend it as later tickets call more methods.
 */
export const ENGINE_INFO = "EngineInfo";
export const ENGINE_READY = "EngineReady";
export const LOCAL_DEVICE = "LocalDevice";
export const WATCH_CHATS = "WatchChats";
export const WATCH_SPACES = "WatchSpaces";
export const WATCH_DEVICES = "WatchDevices";
export const WATCH_SESSIONS = "WatchSessions";
export const WATCH_QUEUE = "WatchQueue";
/** Live edge-connectivity posture (crates/engine/src/rpc.rs:1098): one
 *  `Connectivity` object per engine, re-sent whole on every change. */
export const WATCH_CONNECTIVITY = "WatchConnectivity";
/** The chat doc's transcript stream: full `reset` first, then delta frames. */
export const WATCH_DOC_MESSAGES = "WatchDocMessages";
/** Fetch a tool sidecar blob (`{blobRef}` → `{text}`) - full output/diff text. */
export const FETCH_TOOL_BLOB = "FetchToolBlob";
/** Dev-server discovery for one chat: streams `PreviewSnapshot` (crates/proto/src/preview.rs). */
export const WATCH_PREVIEWS = "WatchPreviews";
/** The one stream that answers a `{stream: true}` readiness ack before items. */
export const WATCH_CHECKOUT_CHANGE_REQUEST = "WatchCheckoutChangeRequest";
/** Harness accounts (legacy wire name "Agent*", ADR 0005); every mutation replies with the fresh snapshot. */
export const LIST_AGENT_ACCOUNTS = "ListAgentAccounts";
export const ACTIVATE_AGENT_ACCOUNT = "ActivateAgentAccount";
export const FORGET_AGENT_ACCOUNT = "ForgetAgentAccount";
export const START_AGENT_LOGIN = "StartAgentLogin";
export const COMPLETE_AGENT_LOGIN = "CompleteAgentLogin";
export const POLL_AGENT_LOGIN = "PollAgentLogin";
export const CANCEL_AGENT_LOGIN = "CancelAgentLogin";
/** Workspace entity mutations, tagged `{op: createChat|renameChat|deleteChat|…}` (crates/engine/src/rpc.rs MutateParams). */
export const MUTATE = "Mutate";
/** The add-space palette's folder browse (`{query, path?, targetDeviceId?}`;
 *  `path` omitted means "browse home"). Replies `FolderListing`. */
export const LIST_FOLDERS = "ListFolders";
/** The add-space palette's Locations rail: mounted drives/volumes of the
 *  browsed device (`{targetDeviceId?}`). Replies `DriveListing`. */
export const LIST_DRIVES = "ListDrives";
/** Resolve/optionally create a typed project path ON THE OWNING DEVICE
 *  (`{path, createIfMissing, targetDeviceId}` — targetDeviceId required).
 *  Replies `PrepareSpacePathReply` (path, exists, gitDetected). */
export const PREPARE_SPACE_PATH = "PrepareSpacePath";
/** Harness catalog for the pickers (one row per harness). */
export const LIST_HARNESSES = "ListHarnesses";
/** Settings → Agents: flip one harness's enablement; the reply is the
 *  device's fresh `ListHarnesses` catalog (a raced toggle self-corrects). */
export const SET_HARNESS_ENABLED = "SetHarnessEnabled";
/** Settings → Agents session-title pickers (per-device `harness-prefs.json`).
 *  `SetTitleSettings`'s params ARE the settings; both reply with the stored pair. */
export const GET_TITLE_SETTINGS = "GetTitleSettings";
export const SET_TITLE_SETTINGS = "SetTitleSettings";
/** Model catalog for the picked harness (filter input drives refetch on focus). */
export const LIST_MODELS = "ListModels";
/** The composer's `/` discovery (crates/rpc/src/lib.rs:42): harness-advertised
 *  slash commands; `{harness, targetDeviceId?}` → `SlashCommand[]`. Cached
 *  once per harness per composer lifetime, filtered locally per keystroke. */
export const LIST_COMMANDS = "ListCommands";
/** The composer's `@` file-mention search (crates/rpc/src/lib.rs:131):
 *  `{query, chatId? | spaceId?, path?, targetDeviceId?}` → `FileSearchMatch[]`.
 *  Debounced 80ms client-side; one retry after 250ms on transport failure. */
export const SEARCH_FILES = "SearchFiles";
/** Composer surface: QueueCommand takes `{chatId, command, transfers}`; command is one of the SessionCommandPayload variants. */
export const QUEUE_COMMAND = "QueueCommand";
/** Failed-send retry (crates/rpc/src/lib.rs:54): `{chatId}` — the engine
 *  re-issues the chat's dead Run/Steer commands under their original message
 *  ids; the user-entry pre-write dedupes by id, so the optimistic echo acks
 *  without doubling. */
export const RETRY_DELIVERY = "RetryDelivery";
/** Message-queue surface (crates/engine/rpc.rs §3.5). The queue lives on the chat doc;
 *  `WatchQueue` streams `{items}` snapshots, the rest are mutations that require
 *  an explicit ack so a racing device's row never silently moves. Edit leases
 *  gate host-authoritative delivery while a client is editing. */
export const QUEUE_MESSAGE = "QueueMessage";
export const UPDATE_QUEUED_MESSAGE = "UpdateQueuedMessage";
export const MOVE_QUEUED_MESSAGE = "MoveQueuedMessage";
export const REMOVE_QUEUED_MESSAGE = "RemoveQueuedMessage";
export const SEND_QUEUED_MESSAGE_NOW = "SendQueuedMessageNow";
export const STEER_QUEUED_MESSAGE_NOW = "SteerQueuedMessageNow";
export const BEGIN_QUEUED_MESSAGE_EDIT = "BeginQueuedMessageEdit";
export const RENEW_QUEUED_MESSAGE_EDIT = "RenewQueuedMessageEdit";
export const FINISH_QUEUED_MESSAGE_EDIT = "FinishQueuedMessageEdit";
/** Workspace file surface (crates/engine/src/workspace_files.rs): the space's
 *  directory tree, text/image reads, writes, and the change stream. */
export const LIST_WORKSPACE_DIRECTORY = "ListWorkspaceDirectory";
export const SEARCH_WORKSPACE_FILES = "SearchWorkspaceFiles";
export const READ_WORKSPACE_FILE = "ReadWorkspaceFile";
export const READ_WORKSPACE_IMAGE = "ReadWorkspaceImage";
export const WRITE_WORKSPACE_FILE = "WriteWorkspaceFile";
/** The one workspace stream; items are `WorkspaceFileChanges` frames. */
export const WATCH_WORKSPACE_FILES = "WatchWorkspaceFiles";

/** Uploads / attachments (crates/rpc/src/lib.rs methods module). Chunked
 *  binary → durable host path. `UploadChunk` and `UploadCommit` may take
 *  `targetDeviceId` to forward to the chat's host device (the web composer
 *  always does — uploads do not write to the browser side). */
export const UPLOAD_CHUNK = "UploadChunk";
export const UPLOAD_COMMIT = "UploadCommit";
/** Transcript image read-back: 64KB base64 chunks until `done`. */
export const READ_ATTACHMENT_CHUNK = "ReadAttachmentChunk";

// Terminals (crates/engine/src/rpc.rs §3.4): OpenTerminal → TerminalSession,
// SubscribeTerminal streams TerminalEvent (replay then live tail, no
// readiness ack — the first item IS the ack), Write/Resize take the
// terminal id, Close kills the PTY.
export const OPEN_TERMINAL = "OpenTerminal";
export const SUBSCRIBE_TERMINAL = "SubscribeTerminal";
export const WRITE_TERMINAL = "WriteTerminal";
export const RESIZE_TERMINAL = "ResizeTerminal";
export const CLOSE_TERMINAL = "CloseTerminal";

/** Per-checkout working-tree diffs (DataRpc, relay-forwardable). */
export const WATCH_CHECKOUT_DIFFS = "WatchCheckoutDiffs";

// Project Actions (crates/engine/src/project_actions.rs): private state on
// the engine owning the space row. Clients call the owning engine's own
// connection (targetDeviceId selects it client-side and is stripped at the
// socket); every mutation replies with the fresh ProjectActionsSnapshot.
export const LIST_PROJECT_ACTIONS = "ListProjectActions";
export const UPSERT_PROJECT_ACTION = "UpsertProjectAction";
export const DELETE_PROJECT_ACTION = "DeleteProjectAction";
/** Launch an action in a managed terminal on the chat's checkout (`{spaceId, chatId, actionId, cols, rows}` → `ProjectActionRun`). */
export const RUN_PROJECT_ACTION = "RunProjectAction";
/** Poll a queued Run's worktree-setup outcome (`{commandId, chatId}` → `{ready, setupAction?, setupError?}`; single-take, 10-min TTL). */
export const TAKE_PROJECT_ACTION_SETUP = "TakeProjectActionSetup";

/**
 * Sidebar organization state (pins + custom sections): engine-local per
 * ADR 0004 (crates/engine/src/sidebar_state.rs), bucketed per workspace
 * profile key — the same buckets the clients compute off the engine's
 * scope + device id. Ordered-list replace, last write wins, every mutation
 * replies with the fresh `SidebarStateSnapshot`, and the watch streams it
 * (current value first) so every client paired to the engine mirrors live.
 * Routing is the local-engine surface: no `targetDeviceId`, no relay.
 */
export const SET_SIDEBAR_PINS = "SetSidebarPins";
export const SET_SIDEBAR_SECTIONS = "SetSidebarSections";
export const WATCH_SIDEBAR_STATE = "WatchSidebarState";

/** Shared remote-safe Git status stream (`{chatId}` → `WorkspaceGitStatusFrame`). */
export const WATCH_WORKSPACE_GIT_STATUS = "WatchWorkspaceGitStatus";
/** One-shot scoped capture (`mode` = workingTree | branch | turn). */
export const GET_CHECKOUT_DIFF = "GetCheckoutDiff";
/** Full text of one side of a file in a diff (used for non-truncated text view). */
export const GET_CHECKOUT_FILE_DIFF_TEXT = "GetCheckoutFileDiffText";
/** Branches for a checkout (one-shot). Default branch first. */
export const LIST_BRANCHES = "ListBranches";
/** Refs for a repo folder (one-shot): branches plus their current/worktree state (`crates/rpc/src/lib.rs:117`, `pickers.rs:1255`). */
export const LIST_REFS = "ListRefs";
/** Check a repo folder out onto another ref (`crates/rpc/src/lib.rs:125`, `pickers.rs:1340`). */
export const SWITCH_REF = "SwitchRef";
/** The History pane's paged commit log (`{cwd, cursor, limit}` → `GitHistoryPage`). */
export const LIST_GIT_HISTORY = "ListGitHistory";
/** Full-repository commit search (`{cwd, query, cursor, limit}` → `GitHistoryPage`). */
export const SEARCH_GIT_HISTORY = "SearchGitHistory";
/** GitHub avatar blobs for commit authors (`{cwd, authors, cursor, limit}` → email → base64). */
export const RESOLVE_GIT_AVATARS = "ResolveGitAvatars";
/** `git fetch --all --quiet` on a repo (`{repoPath}` → `{ok: true}`). */
export const FETCH_ALL = "FetchAll";
