import { useSyncExternalStore } from "react";

/**
 * The sidebar notice strip — the web peer of the desktop's
 * `sidebar_notice`: a failed chat mutation surfaces here and the strip
 * dismisses on click. Module-level so any component running a mutation
 * (row menus, dialogs, the new-chat button) can post to it.
 */
class NoticeStore {
  #notice: string | null = null;
  readonly #listeners = new Set<() => void>();

  getSnapshot(): string | null {
    return this.#notice;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  set(notice: string): void {
    if (notice === this.#notice) {
      return;
    }
    this.#notice = notice;
    for (const listener of this.#listeners) {
      listener();
    }
  }

  clear(): void {
    if (this.#notice === null) {
      return;
    }
    this.#notice = null;
    for (const listener of this.#listeners) {
      listener();
    }
  }
}

export const sidebarNotice = new NoticeStore();

const subscribe = (listener: () => void) => sidebarNotice.subscribe(listener);
const getSnapshot = () => sidebarNotice.getSnapshot();

export function useSidebarNotice(): string | null {
  return useSyncExternalStore(subscribe, getSnapshot);
}
