import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import type { EngineStatus, WatchCacheSnapshot } from "@zeron/engine-client";
import type { EngineSession } from "./engine-session";

/** The live connection status of a session (identity-stable per emission). */
export function useEngineStatus(session: EngineSession | null): EngineStatus | null {
  const subscribe = useCallback(
    (listener: () => void) => (session === null ? () => {} : session.client.onStatus(listener)),
    [session],
  );
  const getSnapshot = useCallback(() => session?.client.status ?? null, [session]);
  // The third arg is the server-render snapshot — same reader, so the
  // surface trees can renderToString (the smoke suite's mount check).
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** The watch cache snapshot of a session, null before a session exists. */
export function useWatchSnapshot(session: EngineSession | null): WatchCacheSnapshot | null {
  const subscribe = useCallback(
    (listener: () => void) => (session === null ? () => {} : session.cache.subscribe(listener)),
    [session],
  );
  const getSnapshot = useCallback(() => session?.cache.getSnapshot() ?? null, [session]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * A ticking clock for staleness gating and relative times — the web peer
 * of the desktop's `Utc::now()` per render.
 */
export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => {
      clearInterval(timer);
    };
  }, [intervalMs]);
  return now;
}
