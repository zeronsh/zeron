import type { Chat, Connectivity, Device, EngineInfo, Session, Space } from "@zeron/proto";
import type { EngineClient, EngineStatus, WatchHandle } from "./client";
import { WATCH_CHATS, WATCH_CONNECTIVITY, WATCH_DEVICES, WATCH_SESSIONS, WATCH_SPACES } from "./methods";
import { RpcError } from "./rpc-error";

/**
 * The watch cache — the web client's single server-state store. It watches
 * the engine's four entity streams (chats, spaces, devices, chat statuses)
 * into one observable cache, mirroring the desktop registry's drive loop
 * (crates/ui/src/engine_registry.rs: verify identity, subscribe every
 * stream, bump the generation per connect) without sharing its code.
 *
 * Contract for React binding (`useSyncExternalStore`, ticket 06):
 * - `getSnapshot()` returns an immutable object whose identity is stable
 *   until an actual change; a re-delivered identical row list never
 *   produces a new identity, so a re-render loop reader always converges;
 * - `subscribe(listener)` fires once per actual change.
 *
 * Semantics:
 * - Every watch item carries the FULL row list for its stream — the
 *   engine's watch channels re-send the complete list on every change.
 *   Rows are applied by key: an unchanged row keeps its object identity,
 *   so both list-level and row-level memoization hold.
 * - On every (re)connect the client re-verifies identity, bumps its
 *   generation, and re-subscribes every stream. At that bump the cache
 *   swaps atomically: all collections reset in one step, so rows from
 *   older generations never survive as ghost state (a chat deleted
 *   server-side while offline must not reappear) and readers never see a
 *   half-swapped cache. Reads made while offline keep returning the
 *   last-known rows, frozen. The re-subscription's first item restores
 *   every row, so the swap loses nothing.
 * - A stream the engine cannot serve (older engine: `unknown method`) or
 *   one that ends with an error degrades only that collection
 *   (`RowSet.error`); the rest of the cache keeps working, and the next
 *   reconnect retries the stream exactly once per generation.
 * - `capabilities` mirrors the connected engine's
 *   `EngineInfo.capabilities`, empty when an older engine omits the
 *   field. No entity stream is capability-gated today; later layers gate
 *   optional features (the message-queue family) through `supports()`.
 */

/** Canonical name for the run-status row the wire still calls `Session` (ADR 0005). */
export type ChatStatus = Session;

export type WatchCollection = "chats" | "spaces" | "devices" | "statuses" | "connectivity";

/** One collection of rows plus its stream health on the current generation. */
export interface RowSet<T> {
  readonly rows: readonly T[];
  /** A first item has arrived on the current generation. */
  readonly loaded: boolean;
  /** Set when the stream degraded: engine lacks the method, stream error, park, or close. */
  readonly error: RpcError | null;
}

/**
 * The connectivity stream's single value plus its health (`WatchConnectivity`
 * streams ONE `Connectivity` object per engine — not a row collection — so it
 * gets a dedicated slot instead of `RowSet`'s multi-row diffing). `loaded` is
 * the web peer of the desktop's `connectivity_observed` (state.rs:1605):
 * false until the first frame of this generation, so a (re)attach re-arms the
 * notification quiet period.
 */
export interface ConnectivitySlot {
  readonly value: Connectivity | null;
  /** A first item has arrived on the current generation. */
  readonly loaded: boolean;
  /** Set when the stream degraded: engine lacks the method, stream error, park, or close. */
  readonly error: RpcError | null;
}

/** The immutable cache snapshot; a fresh identity per actual change, never mutated. */
export interface WatchCacheSnapshot {
  /** The connection generation these rows belong to — the cache-swap epoch. */
  readonly generation: number;
  /** `EngineInfo.capabilities` of the connected engine; empty on older engines. */
  readonly capabilities: readonly string[];
  readonly chats: RowSet<Chat>;
  readonly spaces: RowSet<Space>;
  readonly devices: RowSet<Device>;
  readonly statuses: RowSet<ChatStatus>;
  readonly connectivity: ConnectivitySlot;
}

export interface WatchCacheOptions {
  readonly log?: (message: string, detail?: unknown) => void;
}

interface CollectionState<T> {
  readonly rows: readonly T[];
  readonly loaded: boolean;
  readonly error: RpcError | null;
  readonly rowset: RowSet<T>;
}

interface ConnectivityState {
  readonly value: Connectivity | null;
  readonly loaded: boolean;
  readonly error: RpcError | null;
  readonly slot: ConnectivitySlot;
}

const NO_CAPABILITIES: readonly string[] = [];

export class EngineWatchCache {
  readonly #client: EngineClient;
  readonly #log: (message: string, detail?: unknown) => void;
  #generation = 0;
  #capabilities: readonly string[] = NO_CAPABILITIES;
  #chats: CollectionState<Chat> = emptyCollection();
  #spaces: CollectionState<Space> = emptyCollection();
  #devices: CollectionState<Device> = emptyCollection();
  #statuses: CollectionState<ChatStatus> = emptyCollection();
  #connectivity: ConnectivityState = emptyConnectivity();
  #snapshot: WatchCacheSnapshot;
  readonly #handles: WatchHandle[] = [];
  readonly #listeners = new Set<() => void>();
  readonly #offStatus: () => void;
  #disposed = false;

  /**
   * Attach the cache to a client. The client may be fresh, dialing, or
   * already connected — the four streams are registered immediately and
   * subscribe on the next (or current) connection. A client that is
   * already parked or closed yields a cache whose collections carry that
   * terminal error.
   */
  constructor(client: EngineClient, options: WatchCacheOptions = {}) {
    this.#client = client;
    this.#log = options.log ?? (() => {});
    this.#snapshot = {
      generation: this.#generation,
      capabilities: this.#capabilities,
      chats: this.#chats.rowset,
      spaces: this.#spaces.rowset,
      devices: this.#devices.rowset,
      statuses: this.#statuses.rowset,
      connectivity: this.#connectivity.slot,
    };
    this.#offStatus = client.onStatus((status) => this.#onStatus(status));
    const status = client.status;
    if (status !== null && status.state === "connected") {
      this.#swap(status.info, status.generation);
      this.#registerStreams();
    } else if (client.state === "parked" || client.state === "closed") {
      this.#degradeAll(
        new RpcError(client.state, `Engine client is ${client.state}; the watch cache cannot start`),
      );
    } else {
      this.#registerStreams();
    }
  }

  /** The current snapshot. Identity-stable until an actual change. */
  getSnapshot(): WatchCacheSnapshot {
    return this.#snapshot;
  }

  /** Subscribe to cache changes; the listener fires once per actual change. */
  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Whether the connected engine advertises a capability (always false while unconnected). */
  supports(capability: string): boolean {
    return this.#snapshot.capabilities.includes(capability);
  }

  /** Stop every stream, detach from the client, and freeze the snapshot. */
  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    for (const handle of this.#handles) {
      handle.cancel();
    }
    this.#handles.length = 0;
    this.#offStatus();
    this.#listeners.clear();
  }

  // ── Status and streams ────────────────────────────────────────────────

  #onStatus(status: EngineStatus): void {
    if (this.#disposed) {
      return;
    }
    if (status.state === "connected") {
      this.#swap(status.info, status.generation);
    }
  }

  #swap(info: EngineInfo, generation: number): void {
    this.#generation = generation;
    this.#capabilities = [...(info.capabilities ?? [])];
    this.#chats = emptyCollection();
    this.#spaces = emptyCollection();
    this.#devices = emptyCollection();
    this.#statuses = emptyCollection();
    this.#connectivity = emptyConnectivity();
    this.#commit();
  }

  #registerStreams(): void {
    this.#handles.push(
      this.#client.watch<Chat[]>(WATCH_CHATS, {}, {
        onItem: (rows, { generation }) => {
          if (this.#accepts(generation)) {
            this.#chats = withRows(this.#chats, rows, (row) => row.id);
            this.#commit();
          }
        },
        onEnd: (error) => this.#streamEnded("chats", error),
      }),
      this.#client.watch<Space[]>(WATCH_SPACES, {}, {
        onItem: (rows, { generation }) => {
          if (this.#accepts(generation)) {
            this.#spaces = withRows(this.#spaces, rows, (row) => row.id);
            this.#commit();
          }
        },
        onEnd: (error) => this.#streamEnded("spaces", error),
      }),
      this.#client.watch<Device[]>(WATCH_DEVICES, {}, {
        onItem: (rows, { generation }) => {
          if (this.#accepts(generation)) {
            this.#devices = withRows(this.#devices, rows, (row) => row.id);
            this.#commit();
          }
        },
        onEnd: (error) => this.#streamEnded("devices", error),
      }),
      this.#client.watch<ChatStatus[]>(WATCH_SESSIONS, {}, {
        onItem: (rows, { generation }) => {
          if (this.#accepts(generation)) {
            this.#statuses = withRows(this.#statuses, rows, (row) => row.chatId);
            this.#commit();
          }
        },
        onEnd: (error) => this.#streamEnded("statuses", error),
      }),
      this.#client.watch<Connectivity>(WATCH_CONNECTIVITY, {}, {
        onItem: (value, { generation }) => {
          if (this.#accepts(generation)) {
            this.#connectivity = withValue(this.#connectivity, value);
            this.#commit();
          }
        },
        onEnd: (error) => this.#streamEnded("connectivity", error),
      }),
    );
  }

  /** Items count only on the generation they arrived on — the ghost guard. */
  #accepts(generation: number): boolean {
    return !this.#disposed && generation === this.#generation;
  }

  #streamEnded(key: WatchCollection, error: RpcError | undefined): void {
    if (this.#disposed || error === undefined) {
      return;
    }
    switch (key) {
      case "chats":
        this.#chats = withError(this.#chats, error);
        break;
      case "spaces":
        this.#spaces = withError(this.#spaces, error);
        break;
      case "devices":
        this.#devices = withError(this.#devices, error);
        break;
      case "statuses":
        this.#statuses = withError(this.#statuses, error);
        break;
      case "connectivity":
        this.#connectivity = withConnectivityError(this.#connectivity, error);
        break;
    }
    this.#commit();
  }

  #degradeAll(error: RpcError): void {
    for (const key of ["chats", "spaces", "devices", "statuses", "connectivity"] as const) {
      this.#streamEnded(key, error);
    }
  }

  #commit(): void {
    const next: WatchCacheSnapshot = {
      generation: this.#generation,
      capabilities: this.#capabilities,
      chats: this.#chats.rowset,
      spaces: this.#spaces.rowset,
      devices: this.#devices.rowset,
      statuses: this.#statuses.rowset,
      connectivity: this.#connectivity.slot,
    };
    const previous = this.#snapshot;
    if (
      previous.generation === next.generation &&
      previous.capabilities === next.capabilities &&
      previous.chats === next.chats &&
      previous.spaces === next.spaces &&
      previous.devices === next.devices &&
      previous.statuses === next.statuses &&
      previous.connectivity === next.connectivity
    ) {
      return;
    }
    this.#snapshot = next;
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        this.#log("watch cache listener threw", describeError(error));
      }
    }
  }
}

function emptyCollection<T>(): CollectionState<T> {
  const rows: readonly T[] = [];
  return { rows, loaded: false, error: null, rowset: { rows, loaded: false, error: null } };
}

function emptyConnectivity(): ConnectivityState {
  return { value: null, loaded: false, error: null, slot: { value: null, loaded: false, error: null } };
}

/** Apply one whole-value `Connectivity` frame; identity-stable when unchanged. */
function withValue(state: ConnectivityState, value: Connectivity): ConnectivityState {
  if (state.loaded && state.error === null && jsonEqual(state.value, value)) {
    return state;
  }
  return { value, loaded: true, error: null, slot: { value, loaded: true, error: null } };
}

function withConnectivityError(state: ConnectivityState, error: RpcError): ConnectivityState {
  return {
    value: state.value,
    loaded: state.loaded,
    error,
    slot: { value: state.value, loaded: state.loaded, error },
  };
}

function withRows<T>(
  state: CollectionState<T>,
  rows: readonly T[],
  idOf: (row: T) => string,
): CollectionState<T> {
  const next = applyRows(state.rows, rows, idOf);
  if (next === state.rows && state.loaded && state.error === null) {
    return state;
  }
  return { rows: next, loaded: true, error: null, rowset: { rows: next, loaded: true, error: null } };
}

function withError<T>(state: CollectionState<T>, error: RpcError): CollectionState<T> {
  return { rows: state.rows, loaded: state.loaded, error, rowset: { rows: state.rows, loaded: state.loaded, error } };
}

/**
 * Merge one full-list item into the store: rows keep their identity when
 * deep-equal to the row already held under the same key, so an unchanged
 * engine re-delivery yields the SAME array identity. Row order follows the
 * incoming list (the engine's order is authoritative).
 */
function applyRows<T>(current: readonly T[], incoming: readonly T[], idOf: (row: T) => string): readonly T[] {
  if (!Array.isArray(incoming)) {
    return current;
  }
  if (incoming.length === 0) {
    return current.length === 0 ? current : [];
  }
  const byId = new Map<string, T>();
  for (const row of current) {
    byId.set(idOf(row), row);
  }
  const next: T[] = [];
  for (const row of incoming) {
    const existing = byId.get(idOf(row));
    next.push(existing !== undefined && jsonEqual(existing, row) ? existing : row);
  }
  const unchanged = next.length === current.length && next.every((row, index) => row === current[index]);
  return unchanged ? current : next;
}

function jsonEqual(a: unknown, b: unknown): boolean {
  if (a === b) {
    return true;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((value, index) => jsonEqual(value, b[index]));
  }
  if (isPlainObject(a) && isPlainObject(b)) {
    const keys = Object.keys(a);
    return (
      keys.length === Object.keys(b).length &&
      keys.every((key) => Object.hasOwn(b, key) && jsonEqual(a[key], b[key]))
    );
  }
  return false;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
