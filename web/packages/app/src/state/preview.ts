import { useEffect, useState } from "react";
import type { PreviewSnapshot } from "@zeron/proto";
import { methods, type WatchHandle } from "@zeron/engine-client";
import type { EngineSession } from "./engine-session";

/**
 * The live preview snapshot of one chat — `WatchPreviews {chatId}` on the
 * chat's engine. Not part of the watch cache: the stream is parametric
 * per chat and only consumed while the preview panel is open, mirroring
 * the desktop's per-browser-tab subscription (browser/mod.rs
 * watch_previews), including its re-subscribe loop: a stream that ends
 * mid-connection is retried after a beat; a reconnect re-subscribes via
 * the client's watch supervision. Rows are the engine's stable
 * identities; nothing here renames or re-keys them.
 */

export interface PreviewState {
  /** The latest snapshot; null until the stream's first item. */
  readonly snapshot: PreviewSnapshot | null;
  /** Discovery/proxy/stream problem to surface; null when healthy. */
  readonly error: string | null;
  /** No item yet on the current subscription (the "Looking for…" state). */
  readonly loading: boolean;
}

const RESUBSCRIBE_MS = 2_000;

const INITIAL: PreviewState = { snapshot: null, error: null, loading: true };

export function usePreviews(session: EngineSession | null, chatId: string): PreviewState {
  const [state, setState] = useState<PreviewState>(INITIAL);

  useEffect(() => {
    if (session === null) {
      return;
    }
    setState(INITIAL);
    let cancelled = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let handle: WatchHandle | null = null;
    const subscribe = () => {
      handle = session.client.watch<PreviewSnapshot>(methods.WATCH_PREVIEWS, { chatId }, {
        onItem: (snapshot) => {
          if (!cancelled) {
            setState({ snapshot, error: snapshot.error, loading: false });
          }
        },
        onEnd: (error) => {
          if (cancelled) {
            return;
          }
          // An engine too old to know the method never recovers — degrade
          // once instead of polling it forever.
          if (error?.kind === "unknown-method") {
            setState({ snapshot: null, error: error.message, loading: false });
            return;
          }
          // A parked/closed client is being torn down or re-paired; the
          // session effect owns that lifecycle, not this loop.
          if (session.client.state !== "parked" && session.client.state !== "closed") {
            setState({ snapshot: null, error: "Connecting to preview discovery…", loading: false });
            retry = setTimeout(subscribe, RESUBSCRIBE_MS);
          }
        },
      });
    };
    subscribe();
    return () => {
      cancelled = true;
      clearTimeout(retry);
      handle?.cancel();
    };
  }, [session, chatId]);

  return session === null ? { snapshot: null, error: null, loading: false } : state;
}
