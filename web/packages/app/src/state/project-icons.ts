import { useCallback, useSyncExternalStore } from "react";
import { projectIconsStore } from "../lib/project-icons";

/**
 * The sidebar's read of the project-icon cache — `render_project_icon`'s
 * lookup half: the row's monogram stays until the space's artwork lands
 * (the store re-renders its listeners when a probe settles). Null (loading,
 * offline, miss, or no space) means the monogram IS the icon.
 */
export function useProjectIcon(spaceId: string | null): string | null {
  const subscribe = useCallback((listener: () => void) => projectIconsStore.subscribe(listener), []);
  const getSnapshot = useCallback(() => (spaceId === null ? null : projectIconsStore.iconOf(spaceId)), [spaceId]);
  return useSyncExternalStore(subscribe, getSnapshot, () => null);
}
