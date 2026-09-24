// HAND SHIMS — wire shapes with no Rust type to generate from.
//
// Everything in this file is hand-written and deliberately tiny. Each entry
// names the wire site it describes and why the generator (wiregen) cannot
// own it. If a real Rust type appears for one of these, delete the shim and
// let the generated/ directory take over — the freshness gate will cover it.

import type { QueuedMessage } from "./generated/index";

// HAND SHIM — `ChatConfig.modelOptions` / `RunRequest.modelOptions`.
// Rust type: `serde_json::Map<String, Value>` — harness option id -> choice
// id. Strings in practice, but JSON-round-tripped so anything goes. The
// generated types inline this exact shape (`Record<string, unknown>`); this
// alias exists so hand-written code can name it.
export type ModelOptions = Record<string, unknown>;

// HAND SHIM — `ToolCall::Mcp.input` / `ToolCall::Unknown.input`.
// Rust type: `Option<serde_json::Value>` — the full tool input, pre-sanitize.
// What survives into a chat doc is only the subagent-spawn badge keys
// (`SUBAGENT_INPUT_KEEP`, generated in constants.ts). Generated code inlines
// `unknown` for these fields; this alias names the concept.
export type ToolCallInput = unknown;

// HAND SHIM — `WatchQueue` stream item.
// The engine sends an ad-hoc wrapper (`{ items }`) with no named Rust type.
export interface WatchQueueSnapshot {
  items: QueuedMessage[];
}

// HAND SHIM — ad-hoc `serde_json::json!` RPC replies (crates/engine/rpc.rs).
// No Rust types exist; these are the literal reply shapes.
/** `QueueCommand` reply (send/steer/interrupt/respond-input all ride it). */
export interface QueueCommandReply {
  commandId: string;
}
/** `QueueMessage` reply. */
export interface QueueMessageReply {
  id: string;
}
/** `UpdateQueuedMessage` / `MoveQueuedMessage` reply. */
export interface ChangedReply {
  changed: boolean;
}
/** `RemoveQueuedMessage` reply. */
export interface RemovedReply {
  removed: boolean;
}
/** `SendQueuedMessageNow` / `SteerQueuedMessageNow` reply. */
export interface SentReply {
  sent: boolean;
}
/** `UploadChunk`, `WriteTerminal`, `ResizeTerminal`, `CloseTerminal`,
 *  `CancelAgentLogin`, `StopEngine`, `FetchAll`, `DeleteWorktree`, `Mutate`. */
export interface OkReply {
  ok: true;
}
/** `RetryDelivery` reply: an empty object. */
export type EmptyReply = Record<string, never>;
/** `EngineReady` reply (readiness barrier). */
export interface ReadyReply {
  ready: true;
}
/** `LocalDevice` reply. */
export interface DeviceIdReply {
  deviceId: string;
}
/** `SwitchRef` reply. */
export interface SwitchRefReply {
  branch: string;
}
/** `UploadCommit` reply (absolute host path of the committed file). */
export interface UploadCommitReply {
  path: string;
}
/** `FetchToolBlob` reply (full tool output or full diff text). */
export interface FetchToolBlobReply {
  text: string;
}
/** `PrepareSpacePath` reply (`crates/engine/src/space_paths.rs::SpacePath`,
 * camelCase on the wire): the resolved absolute path, whether it exists, and
 * whether it sits inside a git work tree. */
export interface PrepareSpacePathReply {
  path: string;
  exists: boolean;
  gitDetected: boolean;
}
/** `RelayCommand` reply (peer-delivery fallback outcome). */
export interface RelayCommandReply {
  outcome: "duplicate" | "expired" | "superseded" | "executed";
}
/** Stream readiness preamble (e.g. `WatchCheckoutChangeRequest` sends
 *  `{ok: {stream: true}}` before the first item — an ack, not data). */
export interface StreamAck {
  stream: true;
}
/** `ResolveGitAvatars` reply: author email -> base64 PNG bytes. */
export type ResolveGitAvatarsReply = Record<string, string>;
/** `GET /health` reply on the engine listener. */
export interface HealthReply {
  status: "ok";
}
