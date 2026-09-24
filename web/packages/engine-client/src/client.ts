import type { EngineInfo } from "@zeron/proto";
import { ReconnectBackoff, type BackoffOptions } from "./backoff";
import {
  decodeServerMessage,
  encodeAuthEnvelope,
  encodeClientFrame,
  type ClientFrame,
  type ServerFrame,
} from "./codec";
import { ENGINE_INFO } from "./methods";
import { wireParams } from "./request-routing";
import { RpcError, wireError } from "./rpc-error";
import { browserWebSocket, type WebSocketFactory, type WsSocket, type SocketClose } from "./socket";

/** Close code the engine uses to refuse a session credential (crates/engine/src/listener.rs). */
export const CLOSE_UNAUTHORIZED = 4401;
export const CLOSE_REASON_INVALID_CREDENTIAL = "invalid credential";
export const CLOSE_REASON_AUTH_TIMEOUT = "authentication timeout";
export const CLOSE_REASON_SESSION_UNAVAILABLE = "session check unavailable";

export type ParkedReason = "invalid-credential" | "identity-changed";

/**
 * Connection state, one step richer than the desktop registry's
 * Connected/Reconnecting/Off: the web splits "Off" into parked (re-pair
 * required, never retried) and closed (this client is done).
 */
export type EngineClientState = "connecting" | "connected" | "reconnecting" | "parked" | "closed";

export type EngineStatus =
  | { state: "connecting"; attempt: number }
  | { state: "connected"; info: EngineInfo; generation: number }
  | { state: "reconnecting"; lastError: string; generation: number; attempt: number }
  | { state: "parked"; reason: ParkedReason; detail: string }
  | { state: "closed" };

/**
 * The engine closes with 4401 for a refused credential and for transient
 * auth-side conditions; only a refusal is permanent.
 */
function isPermanentRefusal(close: SocketClose): boolean {
  return close.code === CLOSE_UNAUTHORIZED && close.reason === CLOSE_REASON_INVALID_CREDENTIAL;
}

export interface EngineClientOptions {
  /** `ws://` or `wss://` engine endpoint. */
  readonly endpoint: string;
  /** Session credential (the pairing grant's `credential`). */
  readonly credential: string;
  /**
   * The engine identity to verify on connect and every reconnect. Omitted,
   * the first verified engine is pinned — matching pairing-time adoption on
   * the desktop; a later mismatch still parks, identity never drifts silently.
   */
  readonly expectedDeviceId?: string;
  readonly webSocket?: WebSocketFactory;
  readonly backoff?: BackoffOptions;
  /** Unary call timeout (desktop registry parity: 30s). 0 disables. */
  readonly callTimeoutMs?: number;
  /**
   * Extended timeout for long transfers (desktop parity: 900s for methods
   * containing "Clone" or "Fetch").
   */
  readonly longCallTimeoutMs?: number;
  /** Watch subscribe-ack timeout (desktop registry parity: 15s). 0 disables. */
  readonly watchAckTimeoutMs?: number;
  /** Dial timeout before an unopened socket is abandoned. */
  readonly connectTimeoutMs?: number;
  /**
   * The registry engine key this client routes for — set, every `call` and
   * `watch` runs `request-routing.ts::wireParams` first: ScopedId-shaped
   * envelope fields decode back to this engine's raw ids, ids owned by a
   * different engine reject the request client-side (never sent over the
   * wire), and `targetDeviceId` is stripped before the request leaves.
   * Routing ends at the socket — no engine forwards a request on.
   */
  readonly engineKey?: string;
  readonly log?: (message: string, detail?: unknown) => void;
}

const DEFAULT_CALL_TIMEOUT_MS = 30_000;
const DEFAULT_LONG_CALL_TIMEOUT_MS = 900_000;
const DEFAULT_WATCH_ACK_TIMEOUT_MS = 15_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 10_000;
const IDENTITY_TIMEOUT_MS = 10_000;

export interface WatchHandlers<T> {
  /**
   * One item from the stream. `generation` identifies the connection the
   * item arrived on; a cache built on watches swaps atomically by dropping
   * rows from older generations.
   */
  onItem: (item: T, context: { generation: number }) => void;
  /**
   * The current stream instance ended: server `done`, stream error, ack
   * timeout, client close, or park. `undefined` means a clean end. A later
   * reconnect may deliver items again — a network drop does NOT end a watch.
   */
  onEnd?: (error: RpcError | undefined) => void;
}

export interface WatchHandle {
  readonly method: string;
  /** Stop the stream and stop re-subscribing it on future reconnects. */
  cancel(): void;
}

export interface WatchOptions {
  /**
   * Override the subscribe-ack timeout for this watch; 0 disables it.
   * Streams with no readiness frame and possibly-long silences before the
   * first item (e.g. `SubscribeTerminal` resuming from `afterSeq` on an
   * idle shell) must disable it — the desktop uses a plain subscribe with
   * no ack barrier there for exactly that reason.
   */
  readonly ackTimeoutMs?: number;
}

interface WatchRegistration {
  readonly method: string;
  readonly params: unknown;
  readonly onItem: (item: unknown, context: { generation: number }) => void;
  readonly onEnd: (error: RpcError | undefined) => void;
  readonly ackTimeoutMs: number;
  currentId: number | null;
}

type Timer = ReturnType<typeof setTimeout> | undefined;

type Pending =
  | {
      kind: "call";
      resolve: (value: unknown) => void;
      reject: (error: RpcError) => void;
      timer: Timer;
    }
  | { kind: "watch"; token: number; timer: Timer };

type RunState = "idle" | "running" | "parked" | "closed";

/**
 * A supervised connection to one engine: dial, authenticate with the
 * first-frame `Auth` envelope, verify identity, then serve typed calls and
 * re-subscribing watches across reconnects — mirroring the desktop engine
 * registry's supervise/drive loop without sharing its code.
 *
 * Parked is permanent for an instance: build a fresh client after re-pairing.
 */
export class EngineClient {
  readonly endpoint: string;

  readonly #credential: string;
  #expectedDeviceId: string | null;
  readonly #factory: WebSocketFactory;
  readonly #backoff: ReconnectBackoff;
  readonly #callTimeoutMs: number;
  readonly #longCallTimeoutMs: number;
  readonly #watchAckTimeoutMs: number;
  readonly #connectTimeoutMs: number;
  readonly #engineKey: string | undefined;
  readonly #log: (message: string, detail?: unknown) => void;

  #runState: RunState = "idle";
  #socket: WsSocket | null = null;
  #currentDial: number | null = null;
  #establishedDial: number | null = null;
  #dialCounter = 0;
  #dialTimer: Timer;
  #reconnectTimer: Timer;
  #connectedAt = 0;
  #generation = 0;
  #attempt = 0;
  #info: EngineInfo | null = null;
  #nextId = 1;
  #nextWatchToken = 1;
  readonly #pending = new Map<number, Pending>();
  readonly #watches = new Map<number, WatchRegistration>();
  readonly #statusListeners = new Set<(status: EngineStatus) => void>();
  #status: EngineStatus | null = null;

  constructor(options: EngineClientOptions) {
    this.endpoint = options.endpoint;
    this.#credential = options.credential;
    this.#expectedDeviceId = options.expectedDeviceId ?? null;
    this.#factory = options.webSocket ?? browserWebSocket;
    this.#backoff = new ReconnectBackoff(options.backoff);
    this.#callTimeoutMs = options.callTimeoutMs ?? DEFAULT_CALL_TIMEOUT_MS;
    this.#longCallTimeoutMs = options.longCallTimeoutMs ?? DEFAULT_LONG_CALL_TIMEOUT_MS;
    this.#watchAckTimeoutMs = options.watchAckTimeoutMs ?? DEFAULT_WATCH_ACK_TIMEOUT_MS;
    this.#connectTimeoutMs = options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;
    this.#engineKey = options.engineKey;
    this.#log = options.log ?? (() => {});
  }

  /** Current status snapshot; freshly allocated per emission, never mutated. */
  get status(): EngineStatus | null {
    return this.#status;
  }

  /** The last verified engine identity, or null before the first connect. */
  get engineInfo(): EngineInfo | null {
    return this.#info;
  }

  /**
   * The registry engine key this client routes for (its canonical origin),
   * or null for an unkeyed client. Presentation caches that must not collide
   * across engines key off this.
   */
  get engineKey(): string | null {
    return this.#engineKey ?? null;
  }

  /** Increments on every successful (re)connect; the cache swap epoch. */
  get generation(): number {
    return this.#generation;
  }

  get state(): EngineClientState {
    return this.#status?.state ?? "connecting";
  }

  /** Start supervision. A no-op unless the client is fresh. */
  connect(): void {
    if (this.#runState !== "idle") {
      return;
    }
    this.#runState = "running";
    this.#dial();
  }

  onStatus(listener: (status: EngineStatus) => void): () => void {
    this.#statusListeners.add(listener);
    return () => this.#statusListeners.delete(listener);
  }

  async call<T>(method: string, params: unknown = {}): Promise<T> {
    if (this.#runState === "parked") {
      throw new RpcError("parked", "Engine client is not running");
    }
    if (this.#runState === "closed") {
      throw new RpcError("closed", "Engine client is not running");
    }
    if (this.#establishedDial === null || this.#currentDial === null || this.#socket === null) {
      // Includes the fresh-but-not-yet-dialed state: a supervised client
      // that has not started (or is between dials) reads as offline to
      // callers, matching the desktop's `EngineTarget::call` — a registry
      // entry seeds its cache before the first dial, so a call can land in
      // that window.
      throw new RpcError("transport", "Engine is offline; reconnecting");
    }
    if (this.#engineKey !== undefined) {
      // A foreign scoped id throws here — the request never leaves the client.
      wireParams(this.#engineKey, method, params);
    }
    const value = await this.#pendingCall(method, params, this.#timeoutFor(method));
    return value as T;
  }

  /**
   * Register a watch. While connected it subscribes immediately; while
   * offline it waits and subscribes on the next successful connect. Active
   * watches are re-subscribed after every reconnect, once identity has been
   * re-verified.
   */
  watch<T>(method: string, params: unknown, handlers: WatchHandlers<T>, options: WatchOptions = {}): WatchHandle {
    if (this.#engineKey !== undefined) {
      // Same boundary as `call`: decode/reject/strip before the stream
      // leaves. A foreign id throws here, synchronously to the registrant.
      wireParams(this.#engineKey, method, params);
    }
    const token = this.#nextWatchToken++;
    this.#watches.set(token, {
      method,
      params,
      onItem: handlers.onItem as (item: unknown, context: { generation: number }) => void,
      onEnd: handlers.onEnd ?? (() => {}),
      ackTimeoutMs: options.ackTimeoutMs ?? this.#watchAckTimeoutMs,
      currentId: null,
    });
    if (this.#establishedDial !== null && this.#currentDial !== null) {
      this.#subscribeWatch(token);
    }
    return { method, cancel: () => this.#cancelWatch(token) };
  }

  /** Stop supervision, close the socket, and end every watch. */
  close(): void {
    if (this.#runState === "closed") {
      return;
    }
    this.#runState = "closed";
    clearTimeout(this.#reconnectTimer);
    this.#reconnectTimer = undefined;
    const dial = this.#currentDial;
    const socket = this.#socket;
    if (dial !== null) {
      this.#teardown(dial, new RpcError("closed", "Engine client closed"));
    }
    this.#socket = null;
    socket?.close(1000, "client closed");
    for (const entry of this.#watches.values()) {
      entry.currentId = null;
      this.#deliverWatchEnd(entry, new RpcError("closed", "Engine client closed"));
    }
    this.#watches.clear();
    this.#emit({ state: "closed" });
  }

  // ── Dialing and supervision ───────────────────────────────────────────

  #dial(): void {
    if (this.#runState !== "running") {
      return;
    }
    this.#attempt += 1;
    const dial = ++this.#dialCounter;
    // Every dial announces itself: a re-dial must not stay silent until it
    // fails ("reconnecting") or establishes ("connected") — a status-change
    // heal needs an event to consume between a drop and the re-dial's
    // outcome, so every dial emits its own "connecting".
    this.#emit({ state: "connecting", attempt: this.#attempt });
    let socket: WsSocket;
    try {
      socket = this.#factory(this.endpoint);
    } catch (error) {
      this.#dialFailed(dial, `engine dial failed: ${describeError(error)}`);
      return;
    }
    this.#currentDial = dial;
    this.#socket = socket;
    this.#dialTimer = setTimeout(() => {
      if (this.#currentDial !== dial) {
        return;
      }
      this.#log("engine dial timed out", { endpoint: this.endpoint });
      socket.close();
    }, this.#connectTimeoutMs);
    socket.addEventListener("open", () => {
      if (this.#currentDial !== dial) {
        return;
      }
      clearTimeout(this.#dialTimer);
      this.#dialTimer = undefined;
      this.#onOpen(dial, socket);
    });
    socket.addEventListener("message", (event) => {
      if (this.#currentDial !== dial) {
        return;
      }
      this.#onMessage(event);
    });
    socket.addEventListener("error", () => {
      if (this.#currentDial !== dial) {
        return;
      }
      this.#log("engine socket error");
    });
    socket.addEventListener("close", (event) => {
      if (this.#currentDial !== dial) {
        return;
      }
      this.#onClose(dial, event);
    });
  }

  #dialFailed(dial: number, message: string): void {
    if (this.#currentDial === dial) {
      this.#teardown(dial, new RpcError("transport", message));
    }
    if (this.#runState !== "running") {
      return;
    }
    this.#emit({ state: "reconnecting", lastError: message, generation: this.#generation, attempt: this.#attempt + 1 });
    this.#scheduleReconnect(0);
  }

  #onOpen(dial: number, socket: WsSocket): void {
    // The listener authenticates the first line of the first message and
    // discards the rest of it, so the envelope always travels alone. A
    // throwing send means the socket died; its close event drives the retry.
    try {
      // An empty credential means the transport already authenticated: the
      // DeviceRoom relay validates the browser session cookie at the
      // WebSocket upgrade (PR #319's model), so no Auth envelope is sent.
      if (this.#credential.length > 0) {
        socket.send(encodeAuthEnvelope(this.#credential));
      }
    } catch (error) {
      this.#log("engine auth send failed", describeError(error));
      return;
    }
    void this.#verifyIdentity(dial, socket);
  }

  /**
   * EngineInfo doubles as the auth probe: a refused credential closes the
   * socket with 4401 before any reply, so the identity round trip both
   * verifies the engine and proves the session authenticated.
   */
  async #verifyIdentity(dial: number, socket: WsSocket): Promise<void> {
    try {
      const value = await this.#pendingCall(ENGINE_INFO, {}, IDENTITY_TIMEOUT_MS);
      if (this.#currentDial !== dial || this.#runState !== "running") {
        return;
      }
      const info = asEngineInfo(value);
      if (info === null) {
        throw new RpcError("bad-reply", "EngineInfo reply is not shaped as engine info");
      }
      if (this.#expectedDeviceId === null) {
        this.#expectedDeviceId = info.deviceId;
        this.#log("engine identity pinned", { deviceId: info.deviceId });
      } else if (info.deviceId !== this.#expectedDeviceId) {
        this.#park(
          "identity-changed",
          `Engine identity changed; pair again (expected ${this.#expectedDeviceId}, found ${info.deviceId})`,
        );
        return;
      }
      this.#established(dial, info);
    } catch (error) {
      if (this.#currentDial !== dial || this.#runState !== "running") {
        // The close funnel already classified and rescheduled this dial.
        return;
      }
      this.#teardown(dial, error instanceof RpcError ? error : new RpcError("transport", describeError(error)));
      socket.close();
      this.#emit({
        state: "reconnecting",
        lastError: error instanceof RpcError ? error.message : describeError(error),
        generation: this.#generation,
        attempt: this.#attempt + 1,
      });
      this.#scheduleReconnect(0);
    }
  }

  #established(dial: number, info: EngineInfo): void {
    this.#generation += 1;
    this.#establishedDial = dial;
    this.#connectedAt = Date.now();
    this.#info = info;
    this.#attempt = 0;
    this.#emit({ state: "connected", info, generation: this.#generation });
    this.#log("engine connected", { generation: this.#generation, deviceId: info.deviceId });
    for (const token of this.#watches.keys()) {
      this.#subscribeWatch(token);
    }
  }

  #onMessage(event: { data: unknown }): void {
    if (typeof event.data !== "string") {
      return;
    }
    const { frames, malformed } = decodeServerMessage(event.data);
    if (malformed > 0) {
      this.#log("dropped malformed server frames", { count: malformed });
    }
    for (const frame of frames) {
      this.#route(frame);
    }
  }

  #onClose(dial: number, event: SocketClose): void {
    if (this.#currentDial !== dial) {
      return;
    }
    clearTimeout(this.#dialTimer);
    this.#dialTimer = undefined;
    if (isPermanentRefusal(event)) {
      this.#park("invalid-credential", "Session credential was revoked or refused; pair again");
      return;
    }
    if (event.code === CLOSE_UNAUTHORIZED) {
      // "authentication timeout" / "session check unavailable" are transient.
      this.#log("engine refused the auth handshake, retrying", event);
    }
    const established = this.#establishedDial === dial;
    const lifetime = established ? Date.now() - this.#connectedAt : 0;
    this.#teardown(dial, new RpcError("closed", describeClose(event)));
    if (this.#runState !== "running") {
      return;
    }
    this.#emit({
      state: "reconnecting",
      lastError: describeClose(event),
      generation: this.#generation,
      attempt: this.#attempt + 1,
    });
    this.#scheduleReconnect(lifetime);
  }

  #scheduleReconnect(lifetimeMs: number): void {
    if (this.#runState !== "running") {
      return;
    }
    const delay = this.#backoff.nextDelayMs(lifetimeMs);
    this.#reconnectTimer = setTimeout(() => this.#dial(), delay);
  }

  #park(reason: ParkedReason, detail: string): void {
    if (this.#runState !== "running") {
      return;
    }
    this.#runState = "parked";
    clearTimeout(this.#reconnectTimer);
    this.#reconnectTimer = undefined;
    clearTimeout(this.#dialTimer);
    this.#dialTimer = undefined;
    const dial = this.#currentDial;
    const socket = this.#socket;
    if (dial !== null) {
      this.#teardown(dial, new RpcError("parked", detail));
    }
    this.#socket = null;
    socket?.close(CLOSE_UNAUTHORIZED, detail);
    for (const entry of this.#watches.values()) {
      entry.currentId = null;
      this.#deliverWatchEnd(entry, new RpcError("parked", detail));
    }
    this.#watches.clear();
    this.#emit({ state: "parked", reason, detail });
    this.#log("engine connection parked", { reason, detail });
  }

  /**
   * End one dial: detach the socket, fail in-flight calls with `error`, and
   * detach (but keep) watch registrations for re-subscription on the next
   * connect. Every pending timer is cleared.
   */
  #teardown(dial: number, error: RpcError): void {
    this.#currentDial = null;
    this.#socket = null;
    if (this.#establishedDial === dial) {
      this.#establishedDial = null;
    }
    for (const [id, pending] of this.#pending) {
      clearTimeout(pending.timer);
      this.#pending.delete(id);
      if (pending.kind === "call") {
        pending.reject(error);
      } else {
        const entry = this.#watches.get(pending.token);
        if (entry !== undefined) {
          entry.currentId = null;
        }
      }
    }
  }

  // ── Frames ────────────────────────────────────────────────────────────

  #timeoutFor(method: string): number {
    if (method.includes("Clone") || method.includes("Fetch")) {
      return this.#longCallTimeoutMs;
    }
    return this.#callTimeoutMs;
  }

  #pendingCall(method: string, params: unknown, timeoutMs: number): Promise<unknown> {
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      const timer =
        timeoutMs > 0
          ? setTimeout(() => {
              if (!this.#pending.has(id)) {
                return;
              }
              this.#pending.delete(id);
              reject(new RpcError("timeout", `Engine request timed out: ${method}`));
            }, timeoutMs)
          : undefined;
      this.#pending.set(id, { kind: "call", resolve, reject, timer });
      this.#sendFrame({ id, method, params });
    });
  }

  #sendFrame(frame: ClientFrame): void {
    const socket = this.#socket;
    if (socket === null) {
      return;
    }
    try {
      socket.send(encodeClientFrame(frame));
    } catch (error) {
      this.#log("engine send failed", describeError(error));
      this.#sendFailed(frame.id, new RpcError("transport", `send failed: ${describeError(error)}`));
    }
  }

  #sendFailed(id: number, error: RpcError): void {
    const pending = this.#pending.get(id);
    if (pending === undefined) {
      return;
    }
    clearTimeout(pending.timer);
    this.#pending.delete(id);
    if (pending.kind === "call") {
      pending.reject(error);
    } else {
      const entry = this.#watches.get(pending.token);
      if (entry !== undefined) {
        entry.currentId = null;
      }
      this.#deliverWatchEnd(entry, error);
    }
  }

  #route(frame: ServerFrame): void {
    const pending = this.#pending.get(frame.id);
    if (pending === undefined) {
      if (frame.err !== undefined || frame.ok !== undefined || frame.item !== undefined || frame.done) {
        this.#log("dropped server frame with no pending request", frame);
      }
      return;
    }
    if (frame.err !== undefined) {
      clearTimeout(pending.timer);
      this.#pending.delete(frame.id);
      const error = wireError(frame.err);
      if (pending.kind === "call") {
        pending.reject(error);
      } else {
        const entry = this.#watches.get(pending.token);
        if (entry !== undefined) {
          entry.currentId = null;
        }
        this.#deliverWatchEnd(entry, error);
      }
      return;
    }
    if (frame.ok !== undefined) {
      if (pending.kind === "call") {
        clearTimeout(pending.timer);
        this.#pending.delete(frame.id);
        pending.resolve(frame.ok);
      } else {
        // The stream readiness ack (`{stream: true}`) — items may follow.
        clearTimeout(pending.timer);
        pending.timer = undefined;
      }
      return;
    }
    if (frame.item !== undefined) {
      if (pending.kind === "watch") {
        clearTimeout(pending.timer);
        pending.timer = undefined;
        const entry = this.#watches.get(pending.token);
        if (entry !== undefined && entry.currentId === frame.id) {
          this.#deliverWatchItem(entry, frame.item);
        }
      }
      return;
    }
    if (frame.done) {
      if (pending.kind === "watch") {
        clearTimeout(pending.timer);
        this.#pending.delete(frame.id);
        const entry = this.#watches.get(pending.token);
        if (entry !== undefined) {
          entry.currentId = null;
        }
        this.#deliverWatchEnd(entry, undefined);
      }
    }
  }

  // ── Watches ───────────────────────────────────────────────────────────

  #subscribeWatch(token: number): void {
    const entry = this.#watches.get(token);
    if (entry === undefined || entry.currentId !== null || this.#socket === null) {
      return;
    }
    const id = this.#nextId++;
    const timer =
      entry.ackTimeoutMs > 0
        ? setTimeout(() => {
            const pending = this.#pending.get(id);
            if (pending === undefined || pending.kind !== "watch" || pending.token !== token) {
              return;
            }
            this.#pending.delete(id);
            const current = this.#watches.get(token);
            if (current !== undefined) {
              current.currentId = null;
            }
            this.#deliverWatchEnd(current, new RpcError("timeout", `Engine stream acknowledgement timed out: ${entry.method}`));
          }, entry.ackTimeoutMs)
        : undefined;
    this.#pending.set(id, { kind: "watch", token, timer });
    entry.currentId = id;
    this.#sendFrame({ id, method: entry.method, params: entry.params });
  }

  #cancelWatch(token: number): void {
    const entry = this.#watches.get(token);
    if (entry === undefined) {
      return;
    }
    this.#watches.delete(token);
    const id = entry.currentId;
    if (id === null) {
      return;
    }
    entry.currentId = null;
    const pending = this.#pending.get(id);
    if (pending !== undefined && pending.kind === "watch") {
      clearTimeout(pending.timer);
      this.#pending.delete(id);
    }
    this.#sendFrame({ id, cancel: true });
  }

  #deliverWatchItem(entry: WatchRegistration, item: unknown): void {
    try {
      entry.onItem(item, { generation: this.#generation });
    } catch (error) {
      this.#log("watch item handler threw", describeError(error));
    }
  }

  #deliverWatchEnd(entry: WatchRegistration | undefined, error: RpcError | undefined): void {
    if (entry === undefined) {
      return;
    }
    try {
      entry.onEnd(error);
    } catch (error) {
      this.#log("watch end handler threw", describeError(error));
    }
  }

  #emit(status: EngineStatus): void {
    this.#status = status;
    for (const listener of this.#statusListeners) {
      try {
        listener(status);
      } catch (error) {
        this.#log("status listener threw", describeError(error));
      }
    }
  }
}

function asEngineInfo(value: unknown): EngineInfo | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const deviceId = (value as Record<string, unknown>).deviceId;
  return typeof deviceId === "string" ? (value as EngineInfo) : null;
}

function describeClose(event: SocketClose): string {
  const reason = event.reason.length > 0 ? `: ${event.reason}` : "";
  return `Engine connection closed (${event.code})${reason}`;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
