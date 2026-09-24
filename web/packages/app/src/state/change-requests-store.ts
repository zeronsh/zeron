import type { ChangeRequestSummary, Chat, CheckoutChangeRequestStatus } from "@zeron/proto";
import { parseScopedId } from "@zeron/engine-client";
import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import { methods, RpcError } from "@zeron/engine-client";
import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";

/**
 * Per-checkout change-request state for the web client — the web peer of
 * `crates/ui/src/change_requests.rs::ChangeRequestClientState` minus the
 * legacy `source_context`-less fallbacks. Each (device, cwd, branch) tuple
 * subscribes to `WatchCheckoutChangeRequest`; the latest successful snapshot
 * per tuple stays alive until a fresh `change_request: None` clears it
 * (the wire's authoritative successful lookup with no match, see
 * `CheckoutChangeRequestStatus`).
 *
 * A device whose engine rejects the versioned capability (the older-engine
 * `unknown method` path) is recorded as unsupported and never re-subscribed
 * until its version changes — matching the desktop's behavior so a parked
 * `older engine` does not generate a stream error every cycle.
 *
 * The store also remembers the most recent `provider` string the engine
 * reported for each (device, cwd) checkout. The provider is a property of
 * the repo (its remote URL), not the branch — once the engine resolves it
 * for any branch on that checkout, we keep using it for sibling branches
 * whose `changeRequest` came back null so the create-PR affordance always
 * links to the right host. Keyed by `${deviceId}\u0000${cwd}`.
 */

export interface ChangeRequestTarget {
  readonly deviceId: string;
  readonly cwd: string;
  readonly branch: string;
  readonly checkoutId: string | null;
}

export interface ChangeRequestSnapshot {
  readonly supported: boolean;
  /** The latest successful snapshot per target, keyed by target identity. */
  readonly snapshots: ReadonlyMap<string, CheckoutChangeRequestStatus>;
  /** The most recent unsupported rejection per device, if any. */
  readonly unsupported: ReadonlyMap<string, string>;
  /**
   * The most recent provider string the engine reported for each checkout,
   * keyed by `${deviceId}\u0000${cwd}`. `null` when the engine has never
   * resolved a provider for that checkout (e.g. only ever reported
   * `changeRequest: null`); surfaces pass this to the create-URL helper
   * instead of hardcoding github.
   */
  readonly providers: ReadonlyMap<string, string>;
  /** Generation of the most recent item applied (for React binding). */
  readonly generation: number;
}

/**
 * The visible change request for a chat. `null` when none has been observed
 * yet or the branch/cwd pair was never resolved.
 */
export function changeRequestForChat(
  snapshots: ReadonlyMap<string, CheckoutChangeRequestStatus>,
  target: ChangeRequestTarget,
): ChangeRequestSummary | null {
  const snapshot = snapshots.get(keyOf(target));
  return snapshot?.changeRequest ?? null;
}

/** Identity for a (device, cwd, branch) tuple. */
export function keyOf(target: ChangeRequestTarget): string {
  return `${target.deviceId}\u0000${target.cwd}\u0000${target.branch}`;
}

/** Identity for a (device, cwd) checkout — the unit the provider is stable over. */
export function checkoutKey(deviceId: string, cwd: string): string {
  return `${deviceId}\u0000${cwd}`;
}

/**
 * The provider the engine has most recently reported for a checkout, or
 * `null` when none has been observed. Use this to thread the engine-detected
 * provider into the create-URL helper instead of hardcoding "github".
 */
export function providerForCheckout(
  providers: ReadonlyMap<string, string>,
  deviceId: string,
  cwd: string,
): string | null {
  return providers.get(checkoutKey(deviceId, cwd)) ?? null;
}

export interface ChangeRequestsClient {
  call<T>(method: string, params?: unknown): Promise<T>;
  watch<T>(method: string, params: unknown, handlers: {
    onItem: (item: T, context: { generation: number }) => void;
    onEnd?: (error: RpcError | undefined) => void;
  }): WatchHandle;
}

interface WatchRecord {
  readonly target: ChangeRequestTarget;
  readonly handle: WatchHandle;
  /** True while the stream is still pending its first item. */
  inflight: boolean;
}

export class ChangeRequestStore {
  readonly #client: ChangeRequestsClient;
  readonly #log: (message: string, detail?: unknown) => void;
  /** The paired engine's own device — targets on it omit `targetDeviceId`. */
  #localDeviceId: string | null;
  #snapshots: Map<string, CheckoutChangeRequestStatus> = new Map();
  #unsupported: Map<string, string> = new Map();
  #providers: Map<string, string> = new Map();
  #targets: Map<string, ChangeRequestTarget> = new Map();
  #watches: Map<string, WatchRecord> = new Map();
  #snapshot: ChangeRequestSnapshot;
  #generation = 0;
  #disposed = false;
  readonly #listeners = new Set<() => void>();

  constructor(
    client: EngineClient | ChangeRequestsClient,
    options: { log?: (message: string, detail?: unknown) => void; localDeviceId?: string | null } = {},
  ) {
    this.#client = client;
    this.#log = options.log ?? (() => {});
    this.#localDeviceId = options.localDeviceId ?? null;
    this.#snapshot = this.#takeSnapshot();
  }

  /** The paired engine's device id, when the session later learns it. */
  setLocalDevice(deviceId: string | null): void {
    if (this.#localDeviceId === deviceId) {
      return;
    }
    this.#localDeviceId = deviceId;
    // Re-arm every watch so its params drop (or gain) `targetDeviceId`.
    // Snapshot first: #startWatch re-inserts the same keys, and a Map
    // yields entries re-added mid-iteration again (an infinite loop).
    for (const [key, record] of [...this.#watches]) {
      record.handle.cancel();
      this.#startWatch(key, record.target);
    }
    this.#commit();
  }

  getSnapshot(): ChangeRequestSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Bring the active target set in line with the given list. Each target
   * gets its own `WatchCheckoutChangeRequest` subscription; targets that
   * stop being requested drop their subscriptions and snapshots.
   */
  setTargets(targets: readonly ChangeRequestTarget[]): void {
    if (this.#disposed) {
      return;
    }
    const next = new Map<string, ChangeRequestTarget>();
    for (const target of targets) {
      if (target.branch.trim().length === 0) {
        continue;
      }
      if (this.#unsupported.has(target.deviceId)) {
        continue;
      }
      const key = keyOf(target);
      if (next.has(key)) {
        continue;
      }
      next.set(key, target);
      this.#targets.set(key, target);
    }
    for (const [key, record] of this.#watches) {
      if (!next.has(key)) {
        record.handle.cancel();
        this.#watches.delete(key);
        this.#snapshots.delete(key);
      }
    }
    for (const [key, target] of next) {
      if (this.#watches.has(key)) {
        continue;
      }
      this.#startWatch(key, target);
    }
    this.#commit();
  }

  /** Forget an unsupported device's negative cache (a new engine version). */
  clearUnsupportedOnVersionChange(deviceId: string, version: string | null): void {
    const current = this.#unsupported.get(deviceId);
    if (current === undefined) {
      return;
    }
    if (current === (version ?? "")) {
      return;
    }
    this.#unsupported.delete(deviceId);
    this.#commit();
  }

  /** Drop every subscription and snapshot (pair the engine again etc.). */
  reset(): void {
    if (this.#disposed) {
      return;
    }
    for (const record of this.#watches.values()) {
      record.handle.cancel();
    }
    this.#watches.clear();
    this.#targets.clear();
    this.#snapshots.clear();
    this.#unsupported.clear();
    this.#providers.clear();
    this.#commit();
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    for (const record of this.#watches.values()) {
      record.handle.cancel();
    }
    this.#watches.clear();
    this.#listeners.clear();
  }

  #startWatch(key: string, target: ChangeRequestTarget): void {
    const record: WatchRecord = {
      target,
      handle: this.#client.watch<CheckoutChangeRequestStatus | { ok: unknown } | unknown>(
        methods.WATCH_CHECKOUT_CHANGE_REQUEST,
        watchParams(target, this.#localDeviceId),
        {
          onItem: (item, ctx) => this.#onItem(key, target, item, ctx.generation),
          onEnd: (error) => this.#onEnd(key, target, error),
        },
      ),
      inflight: true,
    };
    this.#watches.set(key, record);
  }

  #onItem(key: string, target: ChangeRequestTarget, item: unknown, generation: number): void {
    if (this.#disposed) {
      return;
    }
    if (generation < this.#generation) {
      return;
    }
    if (isStreamAckValue(item)) {
      const record = this.#watches.get(key);
      if (record !== undefined) {
        record.inflight = false;
      }
      return;
    }
    if (typeof item !== "object" || item === null) {
      return;
    }
    const status = item as CheckoutChangeRequestStatus;
    if (typeof status.checkoutId !== "string" || typeof status.cwd !== "string" || typeof status.branch !== "string") {
      return;
    }
    if (status.deviceId !== target.deviceId || status.cwd !== target.cwd || status.branch !== target.branch) {
      return;
    }
    this.#generation = generation;
    this.#snapshots.set(key, status);
    // Remember the engine-detected provider for this checkout so sibling
    // branches whose lookup came back null can still open the right
    // create-URL. Only overwrite when we got a non-empty value.
    const provider = status.changeRequest?.provider;
    if (typeof provider === "string" && provider.trim().length > 0) {
      this.#providers.set(checkoutKey(target.deviceId, target.cwd), provider);
    }
    const record = this.#watches.get(key);
    if (record !== undefined) {
      record.inflight = false;
    }
    this.#commit();
  }

  #onEnd(key: string, target: ChangeRequestTarget, error: RpcError | undefined): void {
    if (this.#disposed || error === undefined) {
      return;
    }
    // Older engines don't implement WatchCheckoutChangeRequest; cache the
    // version so we don't keep retrying, but otherwise treat the device as
    // supported (the desktop's negative-cache invariant).
    const detail = error.message;
    if (error.kind === "unknown-method" || /unknown method/i.test(detail)) {
      this.#unsupported.set(target.deviceId, "");
      const record = this.#watches.get(key);
      if (record !== undefined) {
        record.handle.cancel();
        this.#watches.delete(key);
        this.#snapshots.delete(key);
      }
      this.#log("change requests: device unsupported", { deviceId: target.deviceId });
      this.#commit();
      return;
    }
    this.#log("change requests: stream ended", { key, detail });
    // For other errors the engine will re-deliver on next reconnect.
    this.#commit();
  }

  #takeSnapshot(): ChangeRequestSnapshot {
    return {
      supported: this.#unsupported.size === 0,
      snapshots: this.#snapshots,
      unsupported: this.#unsupported,
      providers: this.#providers,
      generation: this.#generation,
    };
  }

  #commit(): void {
    const next = this.#takeSnapshot();
    if (
      next.snapshots === this.#snapshot.snapshots
      && next.unsupported === this.#snapshot.unsupported
      && next.providers === this.#snapshot.providers
    ) {
      return;
    }
    this.#snapshot = next;
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        this.#log("change request listener threw", error);
      }
    }
  }
}

/**
 * `watch_params` (change_requests.rs:238-284): the wire params for one
 * target. `targetDeviceId` is OMITTED when the target is the local device —
 * the engine resolves its own device without the hop, byte-for-byte desktop
 * parity.
 */
export function watchParams(target: ChangeRequestTarget, localDeviceId: string | null): Record<string, unknown> {
  const params: Record<string, unknown> = {
    cwd: target.cwd,
    branch: target.branch,
  };
  if (target.deviceId !== localDeviceId) {
    params.targetDeviceId = target.deviceId;
  }
  return params;
}

// ---------------------------------------------------------------------------
// Per-chat subscription entry point
// ---------------------------------------------------------------------------

const EMPTY_SNAPSHOT: ChangeRequestSnapshot = {
  supported: true,
  snapshots: new Map(),
  unsupported: new Map(),
  providers: new Map(),
  generation: 0,
};

/**
 * A chat's watch target, or null when the chat has no conversation-owned
 * source context worth a PR lookup (`change_requests.rs::desired_watch_targets`):
 * archived chats and empty-branch rows never subscribe. The watch's `cwd` is
 * `sourceContext.repoRoot` — the repo root, not the chat's working directory
 * — matching the engine's checkout identity (`desired_watch_targets`,
 * research 07 §427).
 */
export function chatChangeRequestTarget(chat: Chat): ChangeRequestTarget | null {
  const source = chat.sourceContext ?? null;
  if (source === null) {
    return null;
  }
  const branch = source.branch.trim();
  if (branch.length === 0) {
    return null;
  }
  return {
    deviceId: chat.deviceId,
    cwd: source.repoRoot,
    branch,
    checkoutId: source.checkoutId,
  };
}

/**
 * Live PR summaries for a set of chats, keyed by chat id — the per-chat
 * subscribe entry point arbitrary callers need (the sidebar's rows; the
 * Changes pane and chat header keep their own single-target stores). Owns
 * one `ChangeRequestStore` over the client, brings its watch targets in
 * line with the visible chats, and re-resolves `changeRequestForChat` per
 * chat on every snapshot so a stale snapshot never leaks through.
 */
export function useChatChangeRequests(
  client: EngineClient | null,
  chats: readonly Chat[],
  localDeviceId: string | null = null,
): ReadonlyMap<string, ChangeRequestSummary> {
  const store = useMemo(
    () => (client === null ? null : new ChangeRequestStore(client, { localDeviceId })),
    [client, localDeviceId],
  );
  useEffect(() => () => {
    store?.dispose();
  }, [store]);

  // The target set as a stable signature — `chats` is a fresh array each
  // render, but identical targets must not re-run the (cheap, but real)
  // watch reconciliation.
  const targets = useMemo(
    () =>
      chats
        .filter((chat) => !chat.archived)
        .map((chat) => chatChangeRequestTarget(chat))
        .filter((target): target is ChangeRequestTarget => target !== null),
    [chats],
  );
  const signature = useMemo(() => targets.map(keyOf).join("\u0000"), [targets]);

  useEffect(() => {
    store?.setTargets(targets);
    // `targets` is captured per-signature; the linter would keep it in deps
    // and re-fire on every render's new array identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, signature]);

  const snapshot = useSyncExternalStore(
    useCallback(
      (listener: () => void) => (store === null ? () => {} : store.subscribe(listener)),
      [store],
    ),
    useCallback(() => store?.getSnapshot() ?? EMPTY_SNAPSHOT, [store]),
  );

  return useMemo(() => {
    const summaries = new Map<string, ChangeRequestSummary>();
    for (const chat of chats) {
      const target = chatChangeRequestTarget(chat);
      if (target === null) {
        continue;
      }
      const summary = changeRequestForChat(snapshot.snapshots, target);
      if (summary !== null) {
        summaries.set(chat.id, summary);
      }
    }
    return summaries;
  }, [snapshot, chats]);
}

// ---------------------------------------------------------------------------
// Fleet entry point (ticket 31): one child store per paired engine
// ---------------------------------------------------------------------------

/**
 * Live PR summaries for a merged, cross-engine chat list — the fleet peer
 * of `useChatChangeRequests`. Chats arrive scoped (the registry's
 * projected rows); each chat's owning engine (parsed off its scoped id)
 * gets its own `ChangeRequestStore` over that engine's client, and the
 * per-chat summaries merge into one map. Watch params carry the scoped
 * `targetDeviceId`, which the owning client's routing layer decodes and
 * strips at the wire — the desktop's per-device routing.
 */
export function useFleetChatChangeRequests(
  sessions: ReadonlyMap<string, { readonly client: ChangeRequestsClient }>,
  chats: readonly Chat[],
): ReadonlyMap<string, ChangeRequestSummary> {
  const store = useMemo(() => new FleetChangeRequestsStore(), []);
  useEffect(() => () => {
    store.dispose();
  }, [store]);
  useEffect(() => {
    store.setSessions(sessions);
  }, [sessions]);
  useEffect(() => {
    store.setChats(chats);
  }, [chats]);
  return useSyncExternalStore(
    useCallback((listener: () => void) => store.subscribe(listener), [store]),
    useCallback(() => store.getSnapshot(), [store]),
    useCallback(() => store.getSnapshot(), [store]),
  );
}

class FleetChangeRequestsStore {
  readonly #stores = new Map<string, ChangeRequestStore>();
  #sessions = new Map<string, { readonly client: ChangeRequestsClient }>();
  #chats: readonly Chat[] = [];
  #signature = "";
  #snapshot: ReadonlyMap<string, ChangeRequestSummary> = new Map();
  #disposed = false;
  readonly #listeners = new Set<() => void>();

  setSessions(sessions: ReadonlyMap<string, { readonly client: ChangeRequestsClient }>): void {
    if (this.#disposed) {
      return;
    }
    let changed = false;
    for (const [key, entry] of sessions) {
      if (this.#sessions.get(key) === entry) {
        continue;
      }
      this.#sessions.set(key, entry);
      this.#stores.get(key)?.dispose();
      this.#stores.delete(key);
      const store = new ChangeRequestStore(entry.client, {
        log: (message, detail) => console.debug("[change-requests]", message, detail),
      });
      store.subscribe(() => this.#commit());
      this.#stores.set(key, store);
      changed = true;
    }
    for (const [key] of [...this.#sessions]) {
      if (!sessions.has(key)) {
        this.#sessions.delete(key);
        this.#stores.get(key)?.dispose();
        this.#stores.delete(key);
        changed = true;
      }
    }
    if (changed) {
      this.#signature = "";
      this.setChats(this.#chats);
      this.#commit();
    }
  }

  setChats(chats: readonly Chat[]): void {
    if (this.#disposed) {
      return;
    }
    // Group by owning engine off the scoped chat ids, then hand each
    // engine's store its own targets (keyed by scoped device ids, unique
    // across engines).
    const byEngine = new Map<string, ChangeRequestTarget[]>();
    for (const chat of chats) {
      const target = chatChangeRequestTarget(chat);
      if (target === null || chat.archived) {
        continue;
      }
      const engine = engineKeyOfChat(chat);
      if (engine === null) {
        continue;
      }
      const list = byEngine.get(engine) ?? [];
      list.push(target);
      byEngine.set(engine, list);
    }
    const signature = [...byEngine.entries()]
      .map(([engine, targets]) => `${engine}\u0000${targets.map(keyOf).join("\u0000")}`)
      .sort()
      .join("\u0001");
    if (signature === this.#signature) {
      return;
    }
    this.#signature = signature;
    this.#chats = chats;
    for (const [engine, targets] of byEngine) {
      this.#stores.get(engine)?.setTargets(targets);
    }
    // Engines with no targets keep their old watches; clear them.
    for (const [engine, store] of this.#stores) {
      if (!byEngine.has(engine)) {
        store.setTargets([]);
      }
    }
    this.#commit();
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  getSnapshot(): ReadonlyMap<string, ChangeRequestSummary> {
    return this.#snapshot;
  }

  dispose(): void {
    this.#disposed = true;
    for (const store of this.#stores.values()) {
      store.dispose();
    }
    this.#stores.clear();
    this.#listeners.clear();
  }

  #commit(): void {
    if (this.#disposed) {
      return;
    }
    const summaries = new Map<string, ChangeRequestSummary>();
    for (const chat of this.#chats) {
      const target = chatChangeRequestTarget(chat);
      if (target === null || chat.archived) {
        continue;
      }
      const engine = engineKeyOfChat(chat);
      const store = engine === null ? undefined : this.#stores.get(engine);
      if (store === undefined) {
        continue;
      }
      const summary = changeRequestForChat(store.getSnapshot().snapshots, target);
      if (summary !== null) {
        summaries.set(chat.id, summary);
      }
    }
    if (
      summaries.size === this.#snapshot.size &&
      [...summaries.entries()].every(([key, value]) => this.#snapshot.get(key) === value)
    ) {
      return;
    }
    this.#snapshot = summaries;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/** The engine a scoped chat id belongs to, or null when unscoped. */
function engineKeyOfChat(chat: Chat): string | null {
  try {
    return parseScopedId(chat.id).engine;
  } catch {
    return null;
  }
}

/**
 * The stream readiness ack — `{ok: {stream: true}}` arrives as a bare
 * object value before any data item. It is an ack, not data, so the store
 * drops it and waits for the first real snapshot.
 */
function isStreamAckValue(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  return (value as Record<string, unknown>).stream === true;
}
