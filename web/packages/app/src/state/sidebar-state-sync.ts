import { engineRegistry } from "./fleet";
import { sidebarPinProfileKey } from "../lib/sidebar-pins";
import { SidebarStateSync } from "../lib/sidebar-state-sync";
import { uiSettings } from "./ui-settings";

/**
 * The app-global sidebar state bridge (ticket 11): one `SidebarStateSync`
 * over the `ui-settings` store, driven by the engine registry — the web
 * peer of the desktop's `Shell::sidebar_state_sync`. Every paired engine
 * gets a bridge while it is supervised; a forgotten engine loses its
 * bridge (its cached state stays in `localStorage`, the offline cache).
 *
 * The wiring mirrors `state/fleet.ts`'s registry reconciliation: the
 * registry follows the pairing store, and this follows the registry.
 * Consumers import this module for its side effect; the sidebar keeps
 * reading `sidebarStore` / `uiSettings` unchanged.
 */

export const sidebarStateSync = new SidebarStateSync(uiSettings);

function syncBridges(): void {
  const snapshot = engineRegistry.getSnapshot();
  const present = new Set<string>();
  for (const engine of snapshot.engines) {
    present.add(engine.key);
    const profileKey = sidebarPinProfileKey(
      engine.info?.workspaceScope ?? null,
      engine.info?.deviceId ?? null,
    );
    // The bridge only starts once the engine's identity has landed (the
    // bucket key needs the scope + device id); re-attaching when it does
    // restarts the bridge with the resolved key.
    const client = engineRegistry.clientFor(engine.key);
    if (client === null) {
      sidebarStateSync.detach(engine.key);
      continue;
    }
    sidebarStateSync.attach(engine.key, client, profileKey);
  }
  for (const key of sidebarStateSync.attachedKeys()) {
    if (!present.has(key)) {
      sidebarStateSync.detach(key);
    }
  }
}

engineRegistry.subscribe(syncBridges);
syncBridges();
