/**
 * The engine registry for the web client — the browser-side peer of the
 * desktop's Devices settings. Entries live in browser storage scoped to
 * the serving origin (localStorage): one entry per engine origin, the
 * sign-in credential plus the endpoint. Signing in to an origin again
 * replaces its credential; switching the
 * active engine rebuilds the connection. Accepted wart (spec): another
 * engine's own page starts empty — its storage has no credential.
 */

export interface StoredEngine {
  /** Canonical engine origin — the registry key. */
  readonly baseUrl: string;
  /** The redeemed Session credential. */
  readonly credential: string;
  /** The engine's own name for this session (grant label). */
  readonly label: string;
  readonly sessionId: string;
  readonly pairedAt: number;
  /** Pinned engine identity from the first verified connect; null before. */
  deviceId: string | null;
}

export interface FleetState {
  readonly active: string | null;
  readonly engines: readonly StoredEngine[];
  /**
   * Set when the persisted pairing list (`zeron.fleet.v1`) failed to
   * parse: pairing and persisting refuse until it is repaired, and the
   * damaged bytes are preserved untouched — never overwritten with a
   * blank/default value (`damaged_pairing_config_preserves_local_access_
   * and_original_bytes`, engine_registry/tests.rs:196).
   */
  readonly configurationError: string | null;
}

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export interface SignInInput {
  /** The engine origin to connect to. */
  readonly baseUrl: string;
  /** The session credential: a dev user id or a WorkOS access token. */
  readonly credential: string;
  /** The engine-side label for this browser. */
  readonly label: string;
  /** The signed-in identity — keys the stored entry. */
  readonly sessionId: string;
}

export interface EngineStoreOptions {
  readonly storage?: StorageLike;
  readonly now?: () => number;
}

interface PersistedState {
  readonly version: 1;
  readonly active: string | null;
  readonly engines: readonly StoredEngine[];
}

const STORAGE_KEY = "zeron.fleet.v1";
const EMPTY: FleetState = { active: null, engines: [], configurationError: null };
const CONFIGURATION_ERROR =
  "Saved engine configuration could not be read. The stored value is preserved; pairing is unavailable until it is repaired.";

function memoryStorage(): StorageLike {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.has(key) ? map.get(key)! : null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
  };
}

function defaultStorage(): StorageLike {
  const candidate = (globalThis as { localStorage?: StorageLike }).localStorage;
  return candidate ?? memoryStorage();
}

/** Canonical engine origin key: `http://host:port`, host lowercased. */
export function canonicalBaseUrl(baseUrl: string): string {
  return new URL(baseUrl).origin;
}

/** The engine's WebSocket endpoint: the remote listener serves RPC at `/`. */
export function engineWsEndpoint(baseUrl: string): string {
  const url = new URL(baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = "/";
  return url.toString();
}

/** The host[:port] fragment shown for an engine. */
export function engineHost(baseUrl: string): string {
  const url = new URL(baseUrl);
  return url.port.length > 0 ? `${url.hostname}:${url.port}` : url.hostname;
}

export class EngineStore {
  readonly #storage: StorageLike;
  readonly #now: () => number;
  #state: FleetState = EMPTY;
  #configurationError: string | null = null;
  readonly #listeners = new Set<() => void>();

  constructor(options: EngineStoreOptions = {}) {
    this.#storage = options.storage ?? defaultStorage();
    this.#now = options.now ?? Date.now;
    this.#load();
  }

  getSnapshot(): FleetState {
    return this.#configurationError === null
      ? this.#state
      : { ...this.#state, configurationError: this.#configurationError };
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /**
   * Store a signed-in engine and make it active. The credential comes from
   * the sign-in flow (a dev-mode user id, or a WorkOS access token from the
   * engine's proxied code exchange); signing in to an origin again replaces
   * its credential. Refuses while the persisted configuration is damaged —
   * sign-in stays blocked until the stored value is repaired.
   */
  async signInEngine(input: SignInInput): Promise<StoredEngine> {
    if (this.#configurationError !== null) {
      throw new Error("Saved engine configuration needs repair before signing in");
    }
    const baseUrl = canonicalBaseUrl(input.baseUrl);
    const engine: StoredEngine = {
      baseUrl,
      credential: input.credential,
      label: input.label,
      sessionId: input.sessionId,
      pairedAt: this.#now(),
      deviceId: null,
    };
    const others = this.#state.engines.filter((entry) => entry.baseUrl !== baseUrl);
    this.#setState({ active: baseUrl, engines: [...others, engine].sort((a, b) => a.baseUrl.localeCompare(b.baseUrl)), configurationError: null });
    return engine;
  }

  setActive(baseUrl: string): void {
    if (this.#state.active === baseUrl || !this.#state.engines.some((entry) => entry.baseUrl === baseUrl)) {
      return;
    }
    this.#setState({ ...this.#state, active: baseUrl });
  }

  remove(baseUrl: string): void {
    const engines = this.#state.engines.filter((entry) => entry.baseUrl !== baseUrl);
    if (engines.length === this.#state.engines.length) {
      return;
    }
    const active =
      this.#state.active === baseUrl ? (engines.length > 0 ? engines[0]!.baseUrl : null) : this.#state.active;
    this.#setState({ ...this.#state, active, engines });
  }

  /** Pin the verified engine identity for an entry (post-first-connect). */
  pinDevice(baseUrl: string, deviceId: string): void {
    if (this.#state.engines.some((entry) => entry.baseUrl === baseUrl && entry.deviceId === deviceId)) {
      return;
    }
    const engines = this.#state.engines.map((entry) =>
      entry.baseUrl === baseUrl ? { ...entry, deviceId } : entry,
    );
    this.#setState({ ...this.#state, engines });
  }

  activeEngine(): StoredEngine | null {
    const active = this.#state.active;
    if (active === null) {
      return null;
    }
    return this.#state.engines.find((entry) => entry.baseUrl === active) ?? null;
  }

  #load(): void {
    const raw = this.#storage.getItem(STORAGE_KEY);
    if (raw === null) {
      return;
    }
    try {
      const parsed = JSON.parse(raw) as PersistedState;
      if (parsed.version === 1 && Array.isArray(parsed.engines)) {
        const engines = parsed.engines.filter(
          (entry) =>
            typeof entry?.baseUrl === "string" &&
            typeof entry.credential === "string" &&
            typeof entry.sessionId === "string" &&
            typeof entry.label === "string" &&
            typeof entry.pairedAt === "number",
        );
        const active = typeof parsed.active === "string" && engines.some((entry) => entry.baseUrl === parsed.active) ? parsed.active : engines.length > 0 ? engines[0]!.baseUrl : null;
        this.#state = { active, engines, configurationError: null };
        return;
      }
      // Wrong-shape payload: treat as damaged, preserving the bytes.
    } catch {
      // Unparseable payload: damaged — the stored bytes stay untouched.
    }
    // A damaged-but-present value is NEVER overwritten with a blank one:
    // the store reads empty (nothing connects), pairing is refused, and
    // the raw bytes survive for manual repair.
    this.#state = { active: null, engines: [], configurationError: null };
    this.#configurationError = CONFIGURATION_ERROR;
  }

  #persist(): void {
    if (this.#configurationError !== null) {
      // Persisting a healthy list over damaged bytes would silently destroy
      // whatever the user needs to repair — refuse instead.
      return;
    }
    if (this.#state.engines.length === 0) {
      this.#storage.removeItem(STORAGE_KEY);
      return;
    }
    const persisted: PersistedState = { version: 1, ...this.#state };
    this.#storage.setItem(STORAGE_KEY, JSON.stringify(persisted));
  }

  #setState(state: FleetState): void {
    if (
      state.active === this.#state.active &&
      state.engines === this.#state.engines &&
      state.configurationError === this.#state.configurationError
    ) {
      return;
    }
    this.#state = state;
    this.#persist();
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

/** The device label this browser presents when redeeming a pairing link. */
export function webDeviceLabel(
  navigator?: { readonly userAgentData?: { readonly platform?: string }; readonly platform?: string },
): string {
  const source = navigator ?? (globalThis as { navigator?: { userAgentData?: { platform?: string }; platform?: string } }).navigator;
  const platform =
    source?.userAgentData?.platform ??
    (source?.platform !== undefined && source.platform.length > 0 ? source.platform : null) ??
    "this browser";
  return `Zeron web on ${platform}`;
}
