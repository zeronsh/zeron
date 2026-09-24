import { useSyncExternalStore } from "react";
import { SidebarStore, type SidebarState } from "../lib/sidebar-store";

/**
 * The sidebar UI-state singleton. One per page load; the space filter and
 * last selected space persist in browser storage (the desktop's
 * ui-settings.json peer), the archived disclosure stays in-memory.
 */
export const sidebarStore = new SidebarStore();

const subscribe = (listener: () => void) => sidebarStore.subscribe(listener);
const getSnapshot = () => sidebarStore.getSnapshot();

export function useSidebar(): SidebarState {
  return useSyncExternalStore(subscribe, getSnapshot);
}
