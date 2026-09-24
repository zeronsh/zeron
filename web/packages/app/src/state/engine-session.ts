import { EngineClient, EngineWatchCache } from "@zeron/engine-client";
import type { StoredEngine } from "../lib/engine-store";
import { PickerCatalog } from "./picker-catalog";
import { TranscriptPool } from "./transcript-pool";

/**
 * One supervised connection: the `EngineClient` and its watch cache for one
 * stored engine, plus the picker catalog and recent transcripts. The fleet registry
 * (ticket 31) owns the client and watch cache — it keeps one alive for
 * EVERY paired engine simultaneously — so creating a session composes the
 * registry entry's pieces rather than dialing; disposal releases only what
 * the session layer owns (catalog and recent transcripts). A re-pair or a
 * gate Retry replaces the registry entry, which lands here as a new session. A metadata-only
 * StoredEngine replacement (the identity pin rewrites the stored object)
 * does NOT: the wrapper's `engine` field refreshes in place while the live
 * catalog and transcript pool are retained — the wrapper is metadata;
 * those resources share the session lifetime (`reconcileEngineSessions`, ticket 67).
 */
export interface EngineSession {
  readonly engine: StoredEngine;
  readonly client: EngineClient;
  readonly cache: EngineWatchCache;
  /** Picker catalog (harnesses + models) for the composer. Disposed alongside the session. */
  readonly catalog: PickerCatalog;
  /** Recent live transcripts; retained across routes, released with this connection. */
  readonly transcripts: TranscriptPool;
}

export function engineSessionKey(engine: StoredEngine): string {
  return `${engine.baseUrl}\n${engine.credential}`;
}

/**
 * Wrap a registry-owned client + watch cache in the session surface the app
 * reads (the composer's catalog rides along). The client must already be
 * supervised by the registry — this never dials.
 */
export function createEngineSession(
  engine: StoredEngine,
  client: EngineClient,
  cache: EngineWatchCache,
): EngineSession {
  const catalog = new PickerCatalog(client);
  return { engine, client, cache, catalog, transcripts: new TranscriptPool(client) };
}

/** Release the session layer's own resources; the registry closes the client. */
export function disposeEngineSession(session: EngineSession): void {
  session.catalog.dispose();
  session.transcripts.dispose();
}

/**
 * The registry's CURRENT resources for one engine — null while the registry
 * has not adopted them yet (still spawning) or has already torn them down.
 */
export interface SessionResources {
  readonly client: EngineClient;
  readonly cache: EngineWatchCache;
}

export interface SessionReconciliation {
  /**
   * The map to publish — the PREVIOUS map object when every entry's wrapper
   * is identical, so unchanged registry chatter never churns context
   * identity or resubscribes consumers.
   */
  readonly sessions: ReadonlyMap<string, EngineSession>;
  /**
   * Previous sessions whose catalog NO next entry retains. The WRAPPER is
   * not the owned resource — the catalog is: a metadata-refreshed wrapper
   * (an identity pin rewrites the StoredEngine object) retains its live
   * catalog, so wrapper inequality must never dispose (ticket 67 — that
   * check was the first-pair "loading forever" defect). Dispose each
   * distinct displaced catalog exactly once; client/cache teardown stays
   * with the registry.
   */
  readonly displaced: readonly EngineSession[];
}

/**
 * Reconcile the live session map with the stored engines and the registry's
 * CURRENT resources (ticket 67 §2.2). A session is reused only when its
 * credential AND both resource objects still match the current ones — an
 * engine-gate Retry replaces the registry's client/cache without touching
 * pairing metadata, so credential-only reuse would keep querying a torn-down
 * client. A same-resource metadata change refreshes the wrapper's `engine`
 * field only, retaining the live catalog (pending requests and listeners
 * survive the identity pin). Pure: disposal is the caller's job.
 */
export function reconcileEngineSessions(
  previous: ReadonlyMap<string, EngineSession>,
  engines: readonly StoredEngine[],
  currentResources: (baseUrl: string) => SessionResources | null,
): SessionReconciliation {
  const next = new Map<string, EngineSession>();
  for (const engine of engines) {
    const resources = currentResources(engine.baseUrl);
    if (resources === null) {
      // Not adopted by the registry (yet/anymore): omit the entry. A later
      // registry publication recreates the session; never keep a wrapper
      // referencing resources the registry no longer adopts.
      continue;
    }
    const existing = previous.get(engine.baseUrl);
    if (
      existing !== undefined &&
      existing.engine.credential === engine.credential &&
      existing.client === resources.client &&
      existing.cache === resources.cache
    ) {
      next.set(engine.baseUrl, existing.engine === engine ? existing : { ...existing, engine });
      continue;
    }
    next.set(engine.baseUrl, createEngineSession(engine, resources.client, resources.cache));
  }
  const retained = new Set<PickerCatalog>();
  for (const session of next.values()) {
    retained.add(session.catalog);
  }
  const displaced = [...previous.values()].filter((session) => !retained.has(session.catalog));
  let unchanged = next.size === previous.size;
  if (unchanged) {
    for (const [key, session] of next) {
      if (previous.get(key) !== session) {
        unchanged = false;
        break;
      }
    }
  }
  return { sessions: unchanged ? previous : next, displaced };
}
