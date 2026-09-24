import { useMemo, useSyncExternalStore } from "react";
import type { Chat, Device, Space } from "@zeron/proto";
import {
  EngineRegistry,
  IndexedDbEngineCache,
  encodeScopedId,
  projectRegistrySnapshot,
  fetchBrowserDevices,
  fetchBrowserSession,
  startBrowserLogin,
  RelaySocket,
  relayDeviceUrl,
  type BrowserDevice,
  type EngineRegistrySnapshot,
  type RowSet,
} from "@zeron/engine-client";
import type { ChatStatus, ConnectivitySlot, WatchCacheSnapshot } from "@zeron/engine-client";
import type { StoredEngine } from "../lib/engine-store";

/**
 * The engine fleet is the edge's owner-scoped device list (PR #319's
 * browser-session model): the visitor signs in with WorkOS once —
 * automatically, by redirect — and every engine they own that is connected
 * to the edge appears here on its own. No manual pairing, no pasted
 * addresses, no client-held credentials: the HttpOnly session cookie
 * authenticates the device relay WebSocket at its upgrade.
 *
 * `edgeFleet` is the session + device store; `engineRegistry` is the fleet
 * supervisor — one supervised relay connection per device, all driven
 * simultaneously, merging every engine's rows under scoped ids.
 */

export interface EdgeFleetState {
  readonly session: {
    authenticated: boolean;
    ownerId?: string;
    csrfToken?: string;
    /** The signed-in WorkOS user's profile, when the edge reports one. */
    profile?: { firstName?: string; lastName?: string; email?: string; avatarUrl?: string };
  };
  readonly devices: readonly BrowserDevice[];
  readonly active: string | null;
  readonly error: string | null;
}

const EMPTY: EdgeFleetState = {
  session: { authenticated: false },
  devices: [],
  active: null,
  error: null,
};

let state: EdgeFleetState = EMPTY;
const listeners = new Set<() => void>();

function setState(next: Partial<EdgeFleetState>): void {
  state = { ...state, ...next };
  for (const listener of listeners) {
    listener();
  }
}

/** The fleet store: the browser session and the devices the edge reports. */
export const edgeFleet = {
  getSnapshot(): EdgeFleetState {
    return state;
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};

/** The registry singleton: every discovered device, supervised concurrently. */
export const engineRegistry = new EngineRegistry({
  cache: new IndexedDbEngineCache(),
  webSocket: (url) => new RelaySocket(url),
  log: import.meta.env.DEV ? (message, detail) => console.debug("[fleet]", message, detail) : undefined,
});

function deviceConfigs(devices: readonly BrowserDevice[]) {
  return devices.map((device) => ({
    key: device.id,
    endpoint: relayDeviceUrl(device.id),
    // The relay socket authenticates with the browser session cookie at
    // the upgrade; the client holds no credential.
    credential: "",
    expectedDeviceId: device.id,
  }));
}

/** Reconcile the supervised set with whatever the edge reports. */
function syncRegistry(): void {
  engineRegistry.sync(deviceConfigs(state.devices), null);
}

/** Devices as the fleet-shaped entries the app's surfaces already read. */
function storedEngines(devices: readonly BrowserDevice[], ownerId: string | undefined): readonly StoredEngine[] {
  return devices.map((device) => ({
    baseUrl: device.id,
    credential: "",
    label: device.name ?? device.id,
    sessionId: ownerId ?? "",
    pairedAt: 0,
    deviceId: device.id,
  }));
}

let started = false;
let polling: ReturnType<typeof setInterval> | undefined;

/**
 * The identity-relevant projection of a device list: the fields the
 * derived `StoredEngine`s read (`id`, `name`) plus the owner. Everything
 * else — liveness heartbeats above all — lives in the registry's engine
 * states, not in the fleet snapshot.
 */
function engineListSignature(devices: readonly BrowserDevice[], ownerId: string | undefined): string {
  return `${devices.map((device) => `${device.id}\u0000${device.name ?? ""}`).join("\u0001")}\u0002${ownerId ?? ""}`;
}


async function refreshDevices(): Promise<void> {
  try {
    const devices = await fetchBrowserDevices();
    const online = devices.filter((device) => device.online);
    const active =
      state.active !== null && devices.some((device) => device.id === state.active)
        ? state.active
        : (online[0]?.id ?? devices[0]?.id ?? null);
    // An unchanged poll keeps the snapshot identity: the store's devices
    // array is what `useFleet` derives `engines` from, and a new-but-equal
    // list would churn every reconciled session (and every catalog
    // consumer) every 10 seconds.
    if (state.active === active && state.error === null && engineListSignature(devices, state.session.ownerId) === engineListSignature(state.devices, state.session.ownerId)) {
      return;
    }
    setState({ devices, active, error: null });
    syncRegistry();
  } catch (error) {
    // A 401 here means the session expired mid-flight; the next full page
    // load runs the login gate again. Keep the last-known devices rendered.
    setState({ error: error instanceof Error ? error.message : String(error) });
  }
}

/**
 * Boot the fleet: check the browser session; when signed out, redirect
 * straight to WorkOS (the edge mints the PKCE authorize URL, and the
 * callback lands back on the app root with the session cookie set). When
 * signed in, poll the device list and supervise every device through the
 * relay. Runs once at module load, so every route sits behind the gate.
 */
export function startEdgeFleet(): void {
  if (started) {
    return;
  }
  started = true;
  void (async () => {
    try {
      const session = await fetchBrowserSession();
      setState({ session });
      if (!session.authenticated) {
        const authorizationUrl = await startBrowserLogin();
        window.location.href = authorizationUrl;
        return;
      }
      await refreshDevices();
      polling = setInterval(() => {
        void refreshDevices();
      }, 10_000);
    } catch (error) {
      setState({ error: error instanceof Error ? error.message : String(error) });
    }
  })();
}

/** Switch the active device (the surface an engine-backed view targets). */
export function setActiveDevice(deviceId: string): void {
  if (state.devices.some((device) => device.id === deviceId)) {
    setState({ active: deviceId });
  }
}

/** Sign out of the browser session and reload into the login gate. */
export async function signOut(): Promise<void> {
  const csrfToken = state.session.csrfToken;
  if (csrfToken !== undefined) {
    const { browserLogout } = await import("@zeron/engine-client");
    await browserLogout(csrfToken).catch(() => {});
  }
  window.location.href = "/";
}

startEdgeFleet();
if (typeof window !== "undefined") {
  // A cache write in flight must not be lost when the page goes away — park
  // every entry and flush pending writes (`registry.shutdown`).
  for (const event of ["pagehide", "beforeunload"] as const) {
    window.addEventListener(event, () => {
      void engineRegistry.shutdown();
    });
  }
}

const subscribeFleet = (listener: () => void) => edgeFleet.subscribe(listener);
const getFleetSnapshot = () => edgeFleet.getSnapshot();

export function useFleet(): { active: string | null; engines: readonly StoredEngine[]; session: EdgeFleetState["session"]; configurationError: string | null } {
  const snapshot = useSyncExternalStore(subscribeFleet, getFleetSnapshot, getFleetSnapshot);
  // `engines` must keep its identity while the device list is unchanged:
  // the session provider reconciles on `fleet.engines`, and
  // `reconcileEngineSessions` clones every session whose StoredEngine
  // wrapper identity changed — a fresh array here re-ran that effect on
  // every render, cloning sessions forever (the post-login "Maximum
  // update depth exceeded" loop). Derive only when the devices or the
  // owner actually change.
  const ownerId = snapshot.session.ownerId;
  const engines = useMemo(
    () => storedEngines(snapshot.devices, ownerId),
    [snapshot.devices, ownerId],
  );
  return {
    active: snapshot.active,
    engines,
    session: snapshot.session,
    configurationError: snapshot.error,
  };
}

const subscribeRegistry = (listener: () => void) => engineRegistry.subscribe(listener);
const getRegistrySnapshot = () => engineRegistry.getSnapshot();

/** The registry's live snapshot: one entry per device + its rows. */
export function useFleetRegistry(): EngineRegistrySnapshot {
  return useSyncExternalStore(subscribeRegistry, getRegistrySnapshot, getRegistrySnapshot);
}

const EMPTY_ROWS: RowSet<never> = { rows: [], loaded: false, error: null };
const NEVER_CONNECTED_SLOT: ConnectivitySlot = { value: null, loaded: false, error: null };
const EMPTY_SNAPSHOT: WatchCacheSnapshot = {
  generation: 0,
  capabilities: [],
  chats: EMPTY_ROWS,
  spaces: EMPTY_ROWS,
  devices: EMPTY_ROWS,
  statuses: EMPTY_ROWS,
  connectivity: NEVER_CONNECTED_SLOT,
};

/**
 * The merged view every fleet-aware surface reads: `projected()` over the
 * registry snapshot, shaped exactly like one engine's `WatchCacheSnapshot`
 * so `chatListRows`/`healedSpaceFilter`/`chatPageRow` and friends operate
 * unchanged — just over more rows. Rows carry scoped ids; a request for one
 * is decoded back to its owning engine at the wire boundary.
 */
export function useFleetSnapshot(): WatchCacheSnapshot {
  const registry = useFleetRegistry();
  const active = useFleet().active;
  return useMemo(() => {
    if (registry.engines.length === 0) {
      return EMPTY_SNAPSHOT;
    }
    const projected = projectRegistrySnapshot(registry);
    return {
      generation: registry.engines.reduce((total, engine) => total + engine.generation, 0),
      capabilities:
        registry.engines.find((engine) => engine.key === active)?.info?.capabilities ?? [],
      chats: mergedRowSet(registry.engines.map((engine) => engine.chats), projected.chats),
      spaces: mergedRowSet(registry.engines.map((engine) => engine.spaces), projected.spaces),
      devices: mergedRowSet(registry.engines.map((engine) => engine.devices), projected.devices),
      statuses: mergedRowSet(registry.engines.map((engine) => engine.sessions), projected.sessions as ChatStatus[]),
      connectivity: NEVER_CONNECTED_SLOT,
    };
  }, [registry, active]);
}

function mergedRowSet<T>(
  parts: readonly RowSet<T>[],
  rows: readonly T[],
): RowSet<T> {
  const loaded = parts.some((part) => part.loaded);
  // A failed PART degrades only its own engine (badge-level, the entry's
  // lastError); the merged list renders whatever rows exist — a parked or
  // reconnecting engine's cached rows must not be blanked by its own
  // stream error (§2.2: last-known rows keep rendering). The error note
  // surfaces only when there is nothing left to show at all.
  const error =
    rows.length === 0 ? (parts.find((part) => part.error !== null)?.error ?? null) : null;
  return { rows, loaded, error };
}

/**
 * The `EngineConnectionState` of the engine a device id resolves to, keyed
 * by device id — the input to `spaceDeviceTag`'s live-presence override
 * (§2.5: a device backed by a supervised engine reports that engine's
 * connection state, which beats the heartbeat heuristic).
 */
export function engineStatesOf(registry: EngineRegistrySnapshot): Map<string, "connected" | "reconnecting" | "off"> {
  return new Map(registry.engines.map((engine) => [engine.key, engine.state]));
}

/**
 * The fleet's "local device" for group promotion: the ACTIVE engine's own
 * device id, scoped so it matches the projected rows' device ids.
 */
export function fleetLocalDeviceId(registry: EngineRegistrySnapshot, active: string | null): string | null {
  const engine = registry.engines.find((entry) => entry.key === active);
  const deviceId = engine?.info?.deviceId ?? null;
  return engine !== undefined && deviceId !== null ? encodeScopedId(engine.key, deviceId) : null;
}
