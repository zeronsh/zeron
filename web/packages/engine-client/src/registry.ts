import type { Chat, Device, EngineInfo, Session, Space } from "@zeron/proto";
import type { BackoffOptions } from "./backoff";
import { EngineClient, type EngineStatus } from "./client";
import { browserWebSocket, type WebSocketFactory } from "./socket";
import { EngineWatchCache, type ChatStatus, type RowSet } from "./watch-cache";
import { MemoryEngineCache, type CachedRows, type EngineCacheStore } from "./engine-cache";
import { encodeScopedId } from "./scoped-id";

/**
 * The multi-engine registry — the port of `engine_registry.rs::EngineRegistry`
 * (`crates/ui/src/engine_registry.rs`). Owns one supervised `EngineClient`
 * (with its own `ReconnectBackoff`) plus one `EngineWatchCache` per paired
 * engine, drives all of them concurrently, and exposes a merged snapshot the
 * sidebar/composer read without knowing which engine owns which row: rows
 * land in one flat namespace under `ScopedId`-encoded ids, and request
 * routing (`request-routing.ts`, wired through each client's `engineKey`)
 * decodes them back to raw ids on the way out.
 *
 * Entry rows persist across reconnects and page reloads: the last-known rows
 * are kept while an engine reconnects (frozen, exactly like the desktop's
 * `EngineSnapshot`) and re-seeded from the offline cache before the first
 * dial of a fresh page load. Cache writes flow through one FIFO chain per
 * engine so out-of-order saves can never regress the persisted rows;
 * `forget`/`shutdown` flush that chain before touching the cache's stored
 * entries (task-cancel → flush → delete, mirroring the desktop's ordering).
 */

/** `EngineConnectionState` (`engine_registry.rs:64-69`). */
export type EngineConnectionState = "connected" | "reconnecting" | "off";

/** One paired engine's registry row (`EngineSnapshot`). */
export interface EngineEntrySnapshot {
  /** The engine's registry key — its canonical origin (`StoredEngine.baseUrl`). */
  readonly key: string;
  readonly info: EngineInfo | null;
  readonly state: EngineConnectionState;
  readonly lastError: string | null;
  /** Increments on every successful (re)connect — the cache-swap epoch. */
  readonly generation: number;
  /** Last-known rows; frozen while the engine is reconnecting or off. */
  readonly chats: RowSet<Chat>;
  readonly spaces: RowSet<Space>;
  readonly devices: RowSet<Device>;
  readonly sessions: RowSet<ChatStatus>;
}

/** The registry's whole state (`RegistrySnapshot`). */
export interface EngineRegistrySnapshot {
  readonly engines: readonly EngineEntrySnapshot[];
  /** Set when the persisted pairing list failed to parse; pairing is blocked until repaired. */
  readonly configurationError: string | null;
}

/** The merged, scope-tagged row lists (`RegistrySnapshot::projected`). */
export interface ProjectedSnapshot {
  readonly chats: readonly Chat[];
  readonly spaces: readonly Space[];
  readonly devices: readonly Device[];
  readonly sessions: readonly Session[];
}

/** One engine the registry should supervise (`SavedEngine`-equivalent). */
export interface RegistryEngineConfig {
  readonly key: string;
  /** `ws://`/`wss://` endpoint. */
  readonly endpoint: string;
  readonly credential: string;
  /** The pinned identity to verify on every (re)connect; null adopts the first. */
  readonly expectedDeviceId?: string | null;
}

export interface EngineRegistryOptions {
  readonly cache?: EngineCacheStore;
  readonly webSocket?: WebSocketFactory;
  /** Injected per-engine backoff overrides (tests drive the timing curve). */
  readonly backoff?: BackoffOptions;
  readonly log?: (message: string, detail?: unknown) => void;
}

/** How long a burst of watch frames is coalesced before one cache save. */
const CACHE_SAVE_DEBOUNCE_MS = 300;

interface Entry {
  readonly key: string;
  readonly endpoint: string;
  readonly credential: string;
  readonly expectedDeviceId: string | null;
  readonly client: EngineClient;
  readonly watchCache: EngineWatchCache;
  readonly offWatchCache: () => void;
  readonly offStatus: () => void;
  state: EngineConnectionState;
  info: EngineInfo | null;
  lastError: string | null;
  generation: number;
  chats: readonly Chat[];
  chatsLoaded: boolean;
  spaces: readonly Space[];
  spacesLoaded: boolean;
  devices: readonly Device[];
  sessions: readonly Session[];
  /** FIFO write chain; `flush` awaits its tail. */
  writes: Promise<void>;
  saveTimer: ReturnType<typeof setTimeout> | undefined;
  forgotten: boolean;
  /** This entry's published snapshot, replaced only on actual change. */
  snapshot: EngineEntrySnapshot | null;
}

export class EngineRegistry {
  readonly #cache: EngineCacheStore;
  readonly #webSocket: WebSocketFactory;
  readonly #backoff: BackoffOptions | undefined;
  readonly #log: (message: string, detail?: unknown) => void;
  readonly #entries = new Map<string, Entry>();
  readonly #listeners = new Set<() => void>();
  #configurationError: string | null = null;
  #snapshot: EngineRegistrySnapshot;
  #shutdown = false;

  constructor(options: EngineRegistryOptions = {}) {
    this.#cache = options.cache ?? new MemoryEngineCache();
    this.#webSocket = options.webSocket ?? browserWebSocket;
    this.#backoff = options.backoff;
    this.#log = options.log ?? (() => {});
    this.#snapshot = { engines: [], configurationError: null };
  }

  /** The current snapshot; identity-stable until an actual change. */
  getSnapshot(): EngineRegistrySnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** The cache the transcript layer persists opened chats through. */
  get cache(): EngineCacheStore {
    return this.#cache;
  }

  clientFor(key: string): EngineClient | null {
    return this.#entries.get(key)?.client ?? null;
  }

  watchCacheFor(key: string): EngineWatchCache | null {
    return this.#entries.get(key)?.watchCache ?? null;
  }

  /**
   * Reconcile the supervised set with the persisted pairing list: new or
   * re-paired engines start supervising immediately (seeded from cache
   * before the first dial), removed engines are forgotten, changed
   * credentials replace the entry. The registry never dials an engine that
   * is not in `configs`.
   */
  sync(configs: readonly RegistryEngineConfig[], configurationError: string | null): void {
    if (this.#shutdown) {
      return;
    }
    const next = new Map(configs.map((config) => [config.key, config]));
    for (const [key, entry] of this.#entries) {
      const config = next.get(key);
      if (config === undefined) {
        // Forgotten: teardown → flush → delete the cache entries.
        void this.forget(key);
        continue;
      }
      if (config.endpoint !== entry.endpoint || config.credential !== entry.credential) {
        // A re-pair replaces the credential: rebuild the whole entry so a
        // parked client (permanent for an instance) starts over fresh. The
        // cache SURVIVES — the last-known rows are still that engine's.
        this.#teardown(entry);
      }
    }
    for (const config of configs) {
      if (!this.#entries.has(config.key)) {
        this.#spawn(config, config.expectedDeviceId ?? null);
      }
    }
    this.#configurationError = configurationError;
    this.#commit();
  }

  /**
   * Stop supervising one engine: abort its connection, dispose its watches,
   * flush every pending cache write, THEN delete its cache entries. The
   * ordering is load-bearing — an in-flight save must never recreate the
   * entry being removed (`forget` in engine_registry.rs:534-558).
   */
  async forget(key: string): Promise<void> {
    await this.#forget(key);
    this.#commit();
  }

  /** Park every entry, close every socket, flush every pending write. */
  async shutdown(): Promise<void> {
    this.#shutdown = true;
    const entries = [...this.#entries.values()];
    for (const entry of entries) {
      this.#teardown(entry);
      entry.state = "off";
    }
    for (const entry of entries) {
      await entry.writes.catch(() => {});
    }
    this.#commit();
  }

  /**
   * Rebuild one engine's entry from scratch — the gate card's Retry. A
   * parked client is permanent for an instance, so a retry is a recreate
   * (fresh client, fresh backoff), never a redial of the parked one. The
   * cache survives; `expectedDeviceId` re-pins the stored identity so a
   * changed engine still parks instead of silently re-adopting.
   */
  restart(key: string, expectedDeviceId?: string | null): void {
    const entry = this.#entries.get(key);
    if (entry === undefined || this.#shutdown) {
      return;
    }
    const expected = expectedDeviceId ?? entry.info?.deviceId ?? null;
    this.#teardown(entry);
    this.#spawn(
      { key, endpoint: entry.endpoint, credential: entry.credential, expectedDeviceId: expected },
      expected,
    );
    this.#commit();
  }

  // ── Entry lifecycle ───────────────────────────────────────────────────

  #spawn(config: RegistryEngineConfig, expectedDeviceId: string | null): void {
    const client = new EngineClient({
      endpoint: config.endpoint,
      credential: config.credential,
      expectedDeviceId: expectedDeviceId ?? undefined,
      webSocket: this.#webSocket,
      backoff: this.#backoff,
      // Routing: this client decodes ScopedId fields in its own calls and
      // rejects ids owned by another engine before they reach the wire.
      engineKey: config.key,
      log: this.#log,
    });
    const watchCache = new EngineWatchCache(client, { log: this.#log });
    const entry: Entry = {
      key: config.key,
      endpoint: config.endpoint,
      credential: config.credential,
      expectedDeviceId,
      client,
      watchCache,
      offWatchCache: watchCache.subscribe(() => this.#onRows(entry)),
      offStatus: client.onStatus((status) => this.#onStatus(entry, status)),
      state: "reconnecting",
      info: null,
      lastError: null,
      generation: 0,
      chats: [],
      chatsLoaded: false,
      spaces: [],
      spacesLoaded: false,
      devices: [],
      sessions: [],
      writes: Promise.resolve(),
      saveTimer: undefined,
      forgotten: false,
      snapshot: null,
    };
    this.#entries.set(config.key, entry);
    // Seed the offline cache BEFORE the first dial so the last-known rows
    // render while the connection establishes (engine_registry.rs:395-404).
    void this.#seed(entry)
      .catch(() => {})
      .finally(() => {
        if (!entry.forgotten && this.#entries.get(config.key) === entry) {
          client.connect();
        }
      });
    this.#commit();
  }

  async #seed(entry: Entry): Promise<void> {
    const rows = await this.#cache.loadRows(entry.key);
    if (entry.forgotten || rows === null) {
      return;
    }
    entry.chats = rows.chats;
    entry.chatsLoaded = true;
    entry.spaces = rows.spaces;
    entry.spacesLoaded = true;
    entry.devices = rows.devices;
    entry.sessions = rows.sessions;
    this.#commit();
  }

  async #forget(key: string): Promise<void> {
    const entry = this.#entries.get(key);
    if (entry === undefined) {
      return;
    }
    this.#teardown(entry);
    // No new save can register once the entry is torn down; joining the
    // drained writes keeps an in-flight save from recreating the stored
    // entries being removed.
    await entry.writes.catch(() => {});
    await this.#cache.forgetEngine(key).catch((error: unknown) => {
      this.#log("engine cache forget failed", error);
    });
  }

  #teardown(entry: Entry): void {
    entry.forgotten = true;
    if (entry.saveTimer !== undefined) {
      clearTimeout(entry.saveTimer);
      entry.saveTimer = undefined;
    }
    this.#entries.delete(entry.key);
    entry.offWatchCache();
    entry.offStatus();
    entry.watchCache.dispose();
    entry.client.close();
    entry.state = "off";
  }

  #onStatus(entry: Entry, status: EngineStatus): void {
    if (this.#entries.get(entry.key) !== entry) {
      return;
    }
    switch (status.state) {
      case "connected":
        entry.state = "connected";
        entry.info = status.info;
        entry.lastError = null;
        entry.generation = status.generation;
        break;
      case "connecting":
      case "reconnecting":
        entry.state = "reconnecting";
        entry.lastError = status.state === "reconnecting" ? status.lastError : entry.lastError;
        break;
      case "parked":
        // Park-on-revoked: permanent for this engine key until an explicit
        // re-pair replaces the entry (a `sync` with a new credential).
        entry.state = "off";
        entry.lastError = status.detail;
        break;
      case "closed":
        entry.state = "off";
        break;
    }
    this.#commit();
  }

  /**
   * Adopt the live cache's delivered rows. A loaded live rowset wins; a
   * swapped-to-empty one (a reconnect in flight) leaves the last-known rows
   * in place — the desktop's `EngineSnapshot` only ever overwrites rows on
   * frames, so a reconnect never blanks the sidebar.
   */
  #onRows(entry: Entry): void {
    if (this.#entries.get(entry.key) !== entry) {
      return;
    }
    const live = entry.watchCache.getSnapshot();
    if (live.chats.loaded) {
      entry.chats = live.chats.rows;
      entry.chatsLoaded = true;
    }
    if (live.spaces.loaded) {
      entry.spaces = live.spaces.rows;
      entry.spacesLoaded = true;
    }
    if (live.devices.loaded) {
      entry.devices = live.devices.rows;
    }
    if (live.statuses.loaded) {
      entry.sessions = live.statuses.rows;
    }
    this.#scheduleSave(entry);
    this.#commit();
  }

  #scheduleSave(entry: Entry): void {
    if (entry.forgotten || entry.saveTimer !== undefined) {
      return;
    }
    entry.saveTimer = setTimeout(() => {
      entry.saveTimer = undefined;
      if (entry.forgotten) {
        return;
      }
      const rows: CachedRows = {
        chats: entry.chats,
        spaces: entry.spaces,
        devices: entry.devices,
        sessions: entry.sessions,
      };
      // One FIFO chain per engine: every save lands after the previous one
      // completes, so the newest rows always win the stored entry.
      entry.writes = entry.writes
        .then(() => this.#cache.saveRows(entry.key, rows))
        .catch((error: unknown) => {
          this.#log("engine cache save failed", error);
        });
    }, CACHE_SAVE_DEBOUNCE_MS);
  }

  #commit(): void {
    const engines = [...this.#entries.values()]
      .sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0))
      .map((entry) => this.#entrySnapshot(entry))
      .filter((engine): engine is EngineEntrySnapshot => engine !== null);
    const next: EngineRegistrySnapshot = { engines, configurationError: this.#configurationError };
    if (
      this.#snapshot.configurationError === next.configurationError &&
      this.#snapshot.engines.length === next.engines.length &&
      this.#snapshot.engines.every((engine, index) => engine === next.engines[index])
    ) {
      return;
    }
    this.#snapshot = next;
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        this.#log("registry listener threw", error);
      }
    }
  }

  /**
   * The entry's published snapshot — replaced only when something actually
   * changed, so a registry commit (which can re-enter the pairing store's
   * listeners and come back as a `sync`) compares clean and never loops.
   */
  #entrySnapshot(entry: Entry): EngineEntrySnapshot | null {
    const live = entry.watchCache.getSnapshot();
    const previous = entry.snapshot;
    const chats = stableRowset(previous?.chats, entry.chats, entry.chatsLoaded, live.chats.error);
    const spaces = stableRowset(previous?.spaces, entry.spaces, entry.spacesLoaded, live.spaces.error);
    const devices = stableRowset(previous?.devices, entry.devices, live.devices.loaded || entry.devices.length > 0, live.devices.error);
    const sessions = stableRowset(previous?.sessions, entry.sessions, live.statuses.loaded || entry.sessions.length > 0, live.statuses.error);
    if (
      previous !== null &&
      previous.info === entry.info &&
      previous.state === entry.state &&
      previous.lastError === entry.lastError &&
      previous.generation === entry.generation &&
      previous.chats === chats &&
      previous.spaces === spaces &&
      previous.devices === devices &&
      previous.sessions === sessions
    ) {
      return previous;
    }
    const next: EngineEntrySnapshot = {
      key: entry.key,
      info: entry.info,
      state: entry.state,
      lastError: entry.lastError,
      generation: entry.generation,
      chats,
      spaces,
      devices,
      sessions,
    };
    entry.snapshot = next;
    return next;
  }
}

/** The previous rowset when nothing changed, a fresh one otherwise. */
function stableRowset<T>(
  previous: RowSet<T> | undefined,
  rows: readonly T[],
  loaded: boolean,
  error: RowSet<T>["error"],
): RowSet<T> {
  if (
    previous !== undefined &&
    previous.rows === rows &&
    previous.loaded === loaded &&
    previous.error === error
  ) {
    return previous;
  }
  return { rows, loaded, error };
}

/**
 * `RegistrySnapshot::projected()` (`engine_registry.rs:96-146`): scope every
 * row's ids via `encodeScopedId(engine.key, id)` and concatenate across
 * engines, so rows from every paired engine land in one flat list. Device
 * timestamps are dropped for non-connected engines — a stale cached
 * heartbeat must never imply a live connection. An engine with no device
 * rows still contributes its own synthesized device ("Remote engine") so
 * device-keyed lookups resolve.
 */
export function projectRegistrySnapshot(snapshot: EngineRegistrySnapshot): ProjectedSnapshot {
  const chats: Chat[] = [];
  const spaces: Space[] = [];
  const devices: Device[] = [];
  const sessions: Session[] = [];
  for (const engine of snapshot.engines) {
    const scope = (id: string): string => encodeScopedId(engine.key, id);
    for (const chat of engine.chats.rows) {
      chats.push(scopeChat(scope, chat));
    }
    for (const space of engine.spaces.rows) {
      spaces.push(scopeSpace(scope, space));
    }
    const engineDevices =
      engine.devices.rows.length === 0 && engine.info !== null
        ? [
            {
              id: engine.info.deviceId,
              name: "Remote engine",
              platform: "",
              lastSeenAt: null,
              createdAt: null,
              version: null,
              capabilities: engine.info.capabilities ?? [],
            } satisfies Device,
          ]
        : engine.devices.rows;
    for (const device of engineDevices) {
      const connected = engine.state === "connected";
      devices.push({
        ...device,
        id: scope(device.id),
        lastSeenAt: connected ? device.lastSeenAt : null,
      });
    }
    for (const session of engine.sessions.rows) {
      sessions.push({
        ...session,
        chatId: scope(session.chatId),
        deviceId: scope(session.deviceId),
      });
    }
  }
  return { chats, spaces, devices, sessions };
}

function scopeChat(scope: (id: string) => string, chat: Chat): Chat {
  const scoped: Chat = { ...chat, id: scope(chat.id), deviceId: scope(chat.deviceId) };
  if (chat.spaceId !== null && chat.spaceId !== undefined) {
    scoped.spaceId = scope(chat.spaceId);
  }
  if (chat.checkoutId !== null && chat.checkoutId !== undefined) {
    scoped.checkoutId = scope(chat.checkoutId);
  }
  if (chat.sourceContext !== null && chat.sourceContext !== undefined) {
    scoped.sourceContext = { ...chat.sourceContext, checkoutId: scope(chat.sourceContext.checkoutId) };
  }
  return scoped;
}

function scopeSpace(scope: (id: string) => string, space: Space): Space {
  const scoped: Space = { ...space, id: scope(space.id), deviceId: scope(space.deviceId) };
  if (space.checkoutId !== null && space.checkoutId !== undefined) {
    scoped.checkoutId = scope(space.checkoutId);
  }
  return scoped;
}
