import { useEffect, useSyncExternalStore } from "react";
import type { ReactNode } from "react";

/**
 * What the active route contributes to the titlebar.
 *
 * On the desktop the shell owns the titlebar outright and reads the selected
 * chat off `AppState` to draw the identity and the trailing pane control.
 * Routing here is TanStack's, so the route publishes instead: the shell keeps
 * the bar (it must overlay both columns), and whichever page is mounted fills
 * the identity and trailing slots through `useTitlebar`.
 *
 * The right pane's controls are deliberately NOT here. They are shell chrome —
 * the shell owns the pane column, its toggle, its surface tabs and its expand
 * button, and reads the owning chat straight off the router. Routing them
 * through this store put them behind an effect, and worse: `useTitlebar`
 * clears on every dep change, so a store carrying the pane's identity
 * momentarily reported "no pane" on each toggle. That unmounted and remounted
 * the whole column, and a remounted element has no previous width to
 * transition from — the pane's glide died, and its header emptied before the
 * box finished collapsing.
 */
export interface Chrome {
  readonly identity: ReactNode;
  /** The route's new-session handler; `null` hides the `+` (the blank canvas). */
  readonly onNewSession: (() => void) | null;
}

const EMPTY: Chrome = {
  identity: undefined,
  onNewSession: null,
};

class ChromeStore {
  #chrome: Chrome = EMPTY;
  readonly #listeners = new Set<() => void>();

  getSnapshot(): Chrome {
    return this.#chrome;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  set(chrome: Chrome): void {
    this.#chrome = chrome;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  clear(): void {
    this.set(EMPTY);
  }
}

export const chromeStore = new ChromeStore();

const subscribe = (listener: () => void) => chromeStore.subscribe(listener);
const getSnapshot = () => chromeStore.getSnapshot();

export function useChrome(): Chrome {
  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Publish this route's titlebar contribution for as long as it is mounted.
 * The caller owns `deps` — the identity is JSX, so it is rebuilt every render
 * and cannot be compared by identity.
 */
export function useTitlebar(build: () => Chrome, deps: readonly unknown[]): void {
  useEffect(() => {
    chromeStore.set(build());
    return () => chromeStore.clear();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}
