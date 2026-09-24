/**
 * `@zeron/engine-client` — the framework-agnostic connection core a browser
 * uses to drive an engine. No React inside; connection rules (backoff,
 * park-on-revoked, identity re-verification) mirror the desktop's engine
 * registry (crates/ui/src/engine_registry.rs) with no code shared.
 */

export {
  EngineClient,
  CLOSE_UNAUTHORIZED,
  CLOSE_REASON_AUTH_TIMEOUT,
  CLOSE_REASON_INVALID_CREDENTIAL,
  CLOSE_REASON_SESSION_UNAVAILABLE,
  type EngineClientOptions,
  type EngineClientState,
  type EngineStatus,
  type ParkedReason,
  type WatchHandle,
  type WatchHandlers,
  type WatchOptions,
} from "./client";
export {
  ReconnectBackoff,
  type BackoffOptions,
} from "./backoff";
export {
  decodeServerMessage,
  encodeAuthEnvelope,
  encodeClientFrame,
  isStreamAck,
  type CallFrame,
  type CancelFrame,
  type ClientFrame,
  type DecodedMessage,
  type ServerFrame,
} from "./codec";
export {
  exchangeSignInCode,
  fetchSignInConfig,
  parseEngineUrl,
  type AuthFetchOptions,
  type SignInConfig,
  type SignInTokens,
} from "./auth";
export { RpcError, wireError, type RpcErrorKind } from "./rpc-error";
export {
  browserWebSocket,
  type SocketClose,
  type WebSocketFactory,
  type WsSocket,
} from "./socket";
export {
  EngineWatchCache,
  type ChatStatus,
  type ConnectivitySlot,
  type RowSet,
  type WatchCacheOptions,
  type WatchCacheSnapshot,
  type WatchCollection,
} from "./watch-cache";
export {
  SCOPED_ID_PREFIX,
  encodeScopedId,
  isScopedId,
  parseScopedId,
  type ScopedId,
} from "./scoped-id";
export { wireParams } from "./request-routing";
export {
  EngineRegistry,
  projectRegistrySnapshot,
  type EngineConnectionState,
  type EngineEntrySnapshot,
  type EngineRegistryOptions,
  type EngineRegistrySnapshot,
  type ProjectedSnapshot,
  type RegistryEngineConfig,
} from "./registry";
export {
  IndexedDbEngineCache,
  MemoryEngineCache,
  type CachedRows,
  type EngineCacheStore,
} from "./engine-cache";
export * as methods from "./methods";

export {
  decodeDeviceFrame,
  encodeDeviceFrame,
  RelaySocket,
  ECHO_KIND,
  ECHO_DEADLINE_MS,
  MAX_BUFFERED,
  MAX_FRAME,
  MAX_OUTBOUND_FRAME,
  PING_INTERVAL_MS,
  PING_TEXT,
  PONG_TEXT,
  RELAY_KIND,
  RPC_KIND,
  SILENCE_LEASE_MS,
  type DeviceFrameHeader,
} from "./device-frame";
export {
  browserLogout,
  fetchBrowserDevices,
  fetchBrowserSession,
  relayDeviceUrl,
  startBrowserLogin,
  type BrowserDevice,
  type BrowserSession,
} from "./edge";
