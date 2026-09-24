import type { HarnessDescriptor, HarnessId, Model } from "@zeron/proto";
import { methods, RpcError, type RpcErrorKind } from "@zeron/engine-client";
import type { EngineClient } from "@zeron/engine-client";
import { inFlightLost } from "../lib/catalog-loading";
import { normalizeModelRows, offeredHarnesses } from "../lib/model-rows";

/**
 * The pickers' data catalog — one per `EngineSession`. Lists harnesses once
 * for the composer (its picker chip row drives all four pickers) and the
 * model catalog per picked harness, cached until invalidated. The engine
 * answers both with unary calls; missing methods degrade the catalog to
 * empty so older engines fail closed instead of crash the pickers.
 *
 * Loading discipline (`pickers.rs:1028-1276`):
 *
 * 1. **Non-forced loads only fire from `Idle`.** An `Error` must not
 *    re-trigger a load from the render loop (it would flip back to `Loading`
 *    before the retry row ever painted, and spam the engine); retry resets
 *    the slot to `Idle` first.
 * 2. **Forced loads** (a picker open, a Settings→Agents toggle) reload
 *    through `Ready`/`Error` too, because the enabled set can move under the
 *    cache.
 * 3. **Stale-while-revalidate:** a forced refresh of an already-`Ready` slot
 *    does NOT flip to `Loading` — the currently-shown rows stay on screen
 *    while the fresh catalog lands.
 *
 * `loadModels` retries a failed `ListModels` twice for the `opencode`
 * harness only, backing off `attempt * 2` seconds (2s, then 4s), keeping one
 * `Loading` slot alive so recovery needs no close/reopen and can't launch
 * duplicate probes (`pickers.rs:1136-1162`).
 *
 * `setTargetDevice` names the device that RUNS the agents when it differs
 * from the connected engine's own — `targetDeviceId` then rides both RPC
 * calls — and invalidates both catalogs (they are per-device), dropping
 * in-flight responses through a bumped epoch.
 *
 * A unary call fired while the socket is still dialing fails immediately
 * (`EngineClient.call` throws "engine offline"); the catalog re-kicks such
 * errored slots on every `connected` status so a full page reload heals
 * without user action. The boot race can also emit `connected` BEFORE the
 * rejection lands (the cache-seeded mount flips the app to ready
 * mid-dial), so any OTHER status change re-arms slots still carrying a
 * connection-level error — the slot never waits for a `connected` that
 * already fired. That re-arm keys on the error's TYPED kind, never the
 * literal message (ticket 61, hole 4): `transport`/`closed` errors died
 * with the connection state, so a later status change is a fair retry
 * moment; a `timeout` (the engine stayed silent) is engine-side and keeps
 * the connected-only heal.
 *
 * The harness slot's in-flight guard has a bounded lifetime
 * (`HARNESS_IN_FLIGHT_MS`, ticket 61, hole 2): a wedged load older than
 * the bound reads as lost — the slot is re-kickable, a new kick
 * supersedes the flight (its late landing is dropped by the flight
 * token), and the 30s unary timeout stops being the only escape.
 *
 * React binding contract (mirrors the watch cache): `getSnapshot` /
 * `subscribe` are identity-stable until an actual change.
 */

export interface LoadableList<T> {
  readonly rows: readonly T[];
  readonly loaded: boolean;
  readonly error: string | null;
  /**
   * The latched error's `RpcError` kind — the STATE the heal's re-arm
   * keys on (connection-level kinds), never the message text (ticket 61,
   * hole 4). The message stays display-only.
   */
  readonly errorKind: RpcErrorKind | null;
  /** A fetch is in flight for a slot with no rows (the skeleton signal). */
  readonly loading: boolean;
  /** Bumped per fetch attempt; React keys off it for refresh-on-focus. */
  readonly generation: number;
}

const EMPTY_HARNESSES: readonly HarnessDescriptor[] = [];
const EMPTY_MODELS: readonly Model[] = [];
/**
 * Identity-stable empty slots: `getModels` hands this same object to every
 * harness with no slot, so React's `useSyncExternalStore` sees one snapshot
 * identity (a fresh object per call reads as an infinite change loop).
 */
const EMPTY_MODEL_LIST: LoadableList<Model> = {
  rows: EMPTY_MODELS,
  loaded: false,
  error: null,
  errorKind: null,
  loading: false,
  generation: 0,
};

/** The opencode cold-start retry ladder: two extra attempts at 2s then 4s. */
const OPENCODE_MAX_ATTEMPTS = 3;

function emptyList<T>(): LoadableList<T> {
  return { rows: [], loaded: false, error: null, errorKind: null, loading: false, generation: 0 };
}

function listWithRows<T>(prev: LoadableList<T>, rows: readonly T[], generation: number): LoadableList<T> {
  return { rows, loaded: true, error: null, errorKind: null, loading: false, generation };
}

function listWithError<T>(prev: LoadableList<T>, message: string, kind: RpcErrorKind): LoadableList<T> {
  return { rows: prev.rows, loaded: prev.loaded, error: message, errorKind: kind, loading: false, generation: prev.generation };
}

function listWithLoading<T>(prev: LoadableList<T>): LoadableList<T> {
  return { rows: prev.rows, loaded: prev.loaded, error: prev.error, errorKind: prev.errorKind, loading: true, generation: prev.generation + 1 };
}

/**
 * The heal's re-arm family (ticket 61, hole 4): the connection-level
 * `RpcError` kinds. The pre-dial offline throw is `transport`
 * (`EngineClient.call`, client.ts:235-242), a failed dial rejects
 * in-flight calls as `transport`, and a mid-call teardown rejects them
 * as `closed` — all of them died with the CONNECTION state, so a later
 * status change is a fair retry moment. `timeout` (the engine stayed
 * silent) and engine-side failures stay on the connected-only heal.
 */
function isConnectionError(kind: RpcErrorKind | null): boolean {
  return kind === "transport" || kind === "closed";
}

/**
 * The heal guard both slot families share (`#retryOfflineSlots`): a slot
 * qualifies when it latched an error without rows, nothing is in flight for
 * it, and — when only connection-latched slots are being re-armed — the
 * error's KIND is connection-level (state, not message text).
 */
function slotNeedsRetry(slot: LoadableList<unknown>, inFlight: boolean, offlineOnly: boolean): boolean {
  return (
    slot.error !== null &&
    !slot.loaded &&
    !inFlight &&
    (!offlineOnly || isConnectionError(slot.errorKind))
  );
}

function isUnknownMethod(error: RpcError): boolean {
  return error.kind === "unknown-method" || /unknown method/i.test(error.message);
}

export interface PickerCatalogOptions {
  readonly log?: (message: string, detail?: unknown) => void;
}

/** The subset of `EngineClient` the catalog needs (tests drive a fake). */
export interface PickerCatalogClient {
  call<T>(method: string, params?: unknown): Promise<T>;
  onStatus?(listener: (status: { state: string }) => void): () => void;
}

export interface LoadOptions {
  /**
   * A forced load skips the `loaded` guard and reloads through `Ready`/
   * `Error` too (a picker open, a settings toggle) without clearing the
   * currently-shown rows — stale-while-revalidate.
   */
  readonly force?: boolean;
}

export class PickerCatalog {
  readonly #client: PickerCatalogClient;
  readonly #log: (message: string, detail?: unknown) => void;
  readonly #offStatus: (() => void) | null;

  #harnesses: LoadableList<HarnessDescriptor> = emptyList<HarnessDescriptor>();
  /**
   * The harness slot's current flight, or null when idle. A flight token,
   * not a boolean (ticket 61, hole 2): a kick that supersedes a lost
   * flight takes ownership, and the superseded flight's late landing is
   * dropped by the token check — the 30s unary timeout stops gating the
   * lattice.
   */
  #harnessesInFlight: number | null = null;
  /** When the current flight started — `inFlightLost`'s clock. */
  #harnessesInFlightSince = 0;
  /**
   * Monotonic flight ids — a superseded flight's token is never reused, so
   * its late landing stays dropped even after later flights settled.
   */
  #harnessesFlightSeq = 0;
  readonly #models = new Map<HarnessId, LoadableList<Model>>();
  readonly #modelsInFlight = new Set<HarnessId>();
  readonly #listeners = new Set<() => void>();
  readonly #modelListeners = new Map<HarnessId, Set<() => void>>();
  /** The device that runs the agents, when it differs from the engine's own. */
  #targetDeviceId: string | null = null;
  /** Bumped on every invalidate so in-flight responses land dropped. */
  #epoch = 0;
  #disposed = false;

  constructor(client: PickerCatalogClient, options: PickerCatalogOptions = {}) {
    this.#client = client;
    this.#log = options.log ?? (() => {});
    // A page-load call races the websocket dial and fails immediately with a
    // transport error; re-kick the errored slots once the engine connects.
    // Any other status change re-arms the slots still carrying that offline
    // error — see `#retryOfflineSlots`.
    this.#offStatus =
      typeof client.onStatus === "function"
        ? client.onStatus((status) => {
            if (status.state === "connected") {
              this.#retryOfflineSlots();
            } else {
              this.#retryOfflineSlots(true);
            }
          })
        : null;
  }

  /** Identity-stable harness list. Call `loadHarnesses` to refresh. */
  getHarnesses(): LoadableList<HarnessDescriptor> {
    return this.#harnesses;
  }

  /** Identity-stable model list for the picked harness. */
  getModels(harness: HarnessId): LoadableList<Model> {
    return this.#models.get(harness) ?? EMPTY_MODEL_LIST;
  }

  /**
   * The device that runs the agents when it differs from the connected
   * engine's own (`pickers.rs::space_target`): harness/model catalogs come
   * from the CLIs, which live on THAT device. Changing it invalidates both
   * catalogs (they are per-device) and re-kicks the harness list.
   */
  setTargetDevice(deviceId: string | null): void {
    if (this.#disposed || deviceId === this.#targetDeviceId) {
      return;
    }
    this.#targetDeviceId = deviceId;
    const hadHarnesses = this.#harnesses.loaded || this.#harnesses.loading || this.#harnesses.error !== null;
    const requestedModels = [...this.#modelsInFlight, ...this.#models.keys()];
    this.invalidate();
    if (hadHarnesses) {
      void this.loadHarnesses();
    }
    for (const harness of requestedModels) {
      void this.loadModels(harness);
    }
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  subscribeModels(harness: HarnessId, listener: () => void): () => void {
    let set = this.#modelListeners.get(harness);
    if (set === undefined) {
      set = new Set();
      this.#modelListeners.set(harness, set);
    }
    set.add(listener);
    return () => {
      set!.delete(listener);
      if (set!.size === 0) {
        this.#modelListeners.delete(harness);
      }
    };
  }

  /**
   * Fire one harness fetch. Non-forced (the render loop's eager kick) only
   * loads an `Idle` slot; forced refreshes reload through `Ready`/`Error`
   * too, keeping loaded rows on screen while the fresh catalog lands. An
   * in-flight load is reused — unless it has outlived
   * `HARNESS_IN_FLIGHT_MS`, in which case it reads as lost (a wedged
   * call, a hung first message) and this kick supersedes it: the fresh
   * flight owns the slot and the lost one's late landing is dropped.
   */
  async loadHarnesses(options: LoadOptions = {}): Promise<void> {
    if (this.#disposed) {
      return;
    }
    const inFlight = this.#harnessesInFlight;
    if (inFlight !== null && !inFlightLost(this.#harnessesInFlightSince, Date.now())) {
      return;
    }
    const current = this.#harnesses;
    const shouldLoad = !current.loaded || current.error !== null || options.force === true;
    if (!shouldLoad) {
      return;
    }
    const flight = ++this.#harnessesFlightSeq;
    this.#harnessesInFlight = flight;
    this.#harnessesInFlightSince = Date.now();
    const epoch = this.#epoch;
    const generation = current.generation + 1;
    // Stale-while-revalidate: only a row-less slot announces Loading.
    if (!current.loaded) {
      this.#harnesses = listWithLoading(current);
      this.#commitHarnesses();
    }
    try {
      const rows = await this.#client.call<HarnessDescriptor[]>(methods.LIST_HARNESSES, this.#targetParams());
      if (this.#flightStale(flight, epoch)) {
        return;
      }
      const arr = Array.isArray(rows) ? rows : [];
      this.#harnesses = listWithRows(this.#harnesses, arr, generation);
      this.#commitHarnesses();
    } catch (error) {
      if (this.#flightStale(flight, epoch)) {
        return;
      }
      const rpcError = error instanceof RpcError ? error : new RpcError("transport", String(error));
      if (isUnknownMethod(rpcError)) {
        // Older engine: degrade to an empty catalog (the pickers stay inert).
        this.#harnesses = listWithRows(this.#harnesses, EMPTY_HARNESSES, generation);
        this.#commitHarnesses();
        return;
      }
      this.#harnesses = listWithError(this.#harnesses, rpcError.message, rpcError.kind);
      this.#commitHarnesses();
    } finally {
      if (this.#harnessesInFlight === flight) {
        this.#harnessesInFlight = null;
        this.#harnessesInFlightSince = 0;
      }
    }
  }

  /** True when `flight` no longer owns the harness slot: invalidated or superseded. */
  #flightStale(flight: number, epoch: number): boolean {
    return this.#disposed || this.#epoch !== epoch || this.#harnessesInFlight !== flight;
  }

  /**
   * Fire one model fetch for `harness`; cached per harness until invalidated.
   * An `opencode` failure retries twice on a 2s/4s backoff, keeping the one
   * `Loading` slot alive (`pickers.rs:1136-1162`).
   */
  async loadModels(harness: HarnessId, options: LoadOptions = {}): Promise<void> {
    if (this.#disposed || this.#modelsInFlight.has(harness)) {
      return;
    }
    const current = this.getModels(harness);
    const shouldLoad = !current.loaded || current.error !== null || options.force === true;
    if (!shouldLoad) {
      return;
    }
    this.#modelsInFlight.add(harness);
    const epoch = this.#epoch;
    const generation = current.generation + 1;
    if (!current.loaded) {
      this.#models.set(harness, listWithLoading(current));
      this.#commitModels(harness);
    }
    try {
      let attempt = 1;
      for (;;) {
        try {
          const rows = await this.#client.call<Model[]>(methods.LIST_MODELS, {
            harness,
            ...this.#targetParams(),
          });
          if (this.#disposed || this.#epoch !== epoch) {
            return;
          }
          const arr = Array.isArray(rows) ? rows : [];
          this.#models.set(
            harness,
            listWithRows(this.getModels(harness), normalizeModelRows(harness, arr), generation),
          );
          this.#commitModels(harness);
          return;
        } catch (error) {
          const rpcError = error instanceof RpcError ? error : new RpcError("transport", String(error));
          if (isUnknownMethod(rpcError)) {
            if (this.#disposed || this.#epoch !== epoch) {
              return;
            }
            this.#models.set(harness, listWithRows(this.getModels(harness), EMPTY_MODELS, generation));
            this.#commitModels(harness);
            return;
          }
          if (harness !== "opencode" || attempt >= OPENCODE_MAX_ATTEMPTS) {
            throw error;
          }
          // A plugin-heavy OpenCode cold start can fail once while caches,
          // MCP servers, or plugin runtimes are still warming. Keep this
          // single Loading slot alive so recovery requires no picker
          // close/reopen and cannot launch duplicate probes.
          this.#log("opencode model discovery failed; retrying automatically", { attempt });
          await delay(attempt * 2_000);
          attempt += 1;
          if (this.#disposed || this.#epoch !== epoch) {
            return;
          }
        }
      }
    } catch (error) {
      if (this.#disposed || this.#epoch !== epoch) {
        return;
      }
      const rpcError = error instanceof RpcError ? error : new RpcError("transport", String(error));
      this.#models.set(harness, listWithError(this.getModels(harness), rpcError.message, rpcError.kind));
      this.#commitModels(harness);
    } finally {
      this.#modelsInFlight.delete(harness);
    }
  }

  /**
   * Kick a model load for the effective harness AND every offered one, in
   * parallel — by the time the user opens the picker (or switches rail
   * tabs) the lists are already there (`prefetch_models`, pickers.rs:1090).
   * Each `loadModels` call is guarded by its slot state, so re-running this
   * is free.
   */
  prefetchModels(force: boolean): void {
    if (this.#disposed) {
      return;
    }
    const targets = new Set<HarnessId>(offeredHarnesses(this.#harnesses.rows).map((d) => d.id));
    if (this.#harnesses.error === null) {
      // The committed chat's harness may be outside the offered set — its
      // models still matter. Best effort: every loaded or in-flight slot.
      for (const harness of this.#models.keys()) {
        targets.add(harness);
      }
    }
    for (const harness of targets) {
      void this.loadModels(harness, { force });
    }
  }

  /** Reset the harness slot to `Idle` (a Retry click; `pickers.rs` `ensure_*`). */
  resetHarnesses(): void {
    this.#harnesses = emptyList<HarnessDescriptor>();
    this.#commitHarnesses();
  }

  /** Reset one model slot to `Idle` (a Retry click). */
  resetModels(harness: HarnessId): void {
    this.#models.set(harness, emptyList<Model>());
    this.#commitModels(harness);
  }

  /**
   * Retry click for the harness/model popover (`pickers.rs:2804` click
   * behavior): reset the harness catalog to `Idle`, clear every model slot,
   * force-reload both.
   */
  retryHarnessCatalog(): void {
    this.resetHarnesses();
    for (const harness of [...this.#models.keys()]) {
      this.resetModels(harness);
    }
    void this.loadHarnesses({ force: true });
    this.prefetchModels(true);
  }

  /** Forget the harness + every model catalog (a fresh chat deserves fresh defaults). */
  invalidate(): void {
    this.#epoch += 1;
    this.#harnesses = emptyList<HarnessDescriptor>();
    this.#harnessesInFlight = null;
    this.#harnessesInFlightSince = 0;
    this.#models.clear();
    this.#modelsInFlight.clear();
    this.#commitHarnesses();
    for (const harness of [...this.#modelListeners.keys()]) {
      this.#commitModels(harness);
    }
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    this.#offStatus?.();
    this.#listeners.clear();
    this.#modelListeners.clear();
    this.#models.clear();
  }

  #targetParams(): Record<string, string> {
    return this.#targetDeviceId === null ? {} : { targetDeviceId: this.#targetDeviceId };
  }

  /**
   * The status heal. `connected` re-kicks every errored, not-loaded slot
   * (the plain-refresh heal). Any other status change — the client is, by
   * definition, not connected — re-arms only slots carrying a
   * connection-level error: such a slot latched while the client's
   * connection state was broken, and a later status change is the next
   * chance to retry ("connected" may already have fired before the
   * rejection landed). The re-arm keys on the error's typed kind, never
   * the message text, so a failed dial (`transport`) and a mid-call
   * teardown (`closed`) re-arm exactly like the pre-dial offline error.
   * A failure that landed while the connection was up (the unary call
   * timeout) is NOT re-armed here — the cadence/focus re-kick or the
   * card-open force heals it, and the chip shows the real label meanwhile.
   * A harness flight past its lifetime bound reads as lost, so a wedged
   * load stops blocking the heal (ticket 61, hole 2).
   */
  #retryOfflineSlots(offlineOnly = false): void {
    if (this.#disposed) {
      return;
    }
    const harnessesInFlight =
      this.#harnessesInFlight !== null && !inFlightLost(this.#harnessesInFlightSince, Date.now());
    if (slotNeedsRetry(this.#harnesses, harnessesInFlight, offlineOnly)) {
      this.resetHarnesses();
      void this.loadHarnesses();
    }
    for (const [harness, slot] of this.#models) {
      if (slotNeedsRetry(slot, this.#modelsInFlight.has(harness), offlineOnly)) {
        this.resetModels(harness);
        void this.loadModels(harness);
      }
    }
  }

  #commitHarnesses(): void {
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        this.#log("picker catalog listener threw", describeError(error));
      }
    }
  }

  #commitModels(harness: HarnessId): void {
    const set = this.#modelListeners.get(harness);
    if (set === undefined) {
      return;
    }
    for (const listener of set) {
      try {
        listener();
      } catch (error) {
        this.#log("picker catalog listener threw", describeError(error));
      }
    }
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
