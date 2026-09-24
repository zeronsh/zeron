import { createContext, useContext } from "react";
import type { QueueStore } from "./queue-store";

/**
 * React binding for the per-chat `QueueStore`. The chat page wraps its
 * subtree in `<QueueStoreProvider value={store}>` so any descendant
 * (the panel, the composer) can read the live queue without prop
 * drilling. The provider never rebuilds the store; chat switches
 * dispose the old store and mount a fresh one in its place.
 */
const QueueStoreContext = createContext<QueueStore | null>(null);

export const QueueStoreProvider = QueueStoreContext.Provider;

/** Read the active chat's queue store, or null when none is mounted. */
export function useQueueStore(): QueueStore {
  const store = useContext(QueueStoreContext);
  if (store === null) {
    throw new Error("useQueueStore must be called inside a QueueStoreProvider");
  }
  return store;
}