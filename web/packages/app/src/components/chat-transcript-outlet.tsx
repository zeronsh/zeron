import { useCallback, useSyncExternalStore, type ReactNode } from "react";
import { type TranscriptStore, transcriptSnapshotIsLive } from "../state/transcript-store";

/** Keeps seed hydration mounted while authoritative arrival owns visibility. */
export function ChatTranscriptOutlet({ store, departing, children }: {
  store: TranscriptStore;
  departing: boolean;
  children: (store: TranscriptStore) => ReactNode;
}) {
  const subscribe = useCallback((listener: () => void) => store.subscribe(listener), [store]);
  const getReady = useCallback(() => transcriptSnapshotIsLive(store.getSnapshot()), [store]);
  const ready = useSyncExternalStore(subscribe, getReady, () => true);
  // Selection changes immediately. Only the destination's presentation
  // waits for live data; retaining the old chat would hide slow arrivals.
  const waiting = !departing && !ready;
  return (
    <div className="chat-transcript-outlet" aria-busy={waiting}>
      <div
        key={store.docId}
        className="chat-arrival-gate"
        style={{ opacity: waiting ? 0 : 1 }}
        aria-hidden={waiting}
        inert={waiting}
      >
        {children(store)}
      </div>
      {waiting && (
        <div className="chat-transcript-loading" role="status">Loading chat…</div>
      )}
    </div>
  );
}
