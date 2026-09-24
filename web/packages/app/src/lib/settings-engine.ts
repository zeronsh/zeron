import type { EngineEntrySnapshot } from "@zeron/engine-client";
import { engineHost, type FleetState } from "./engine-store";

/**
 * The engine-addressing settings vocabulary: which engine the settings
 * pages that silently talk to the *active* one (Remote access, Agents,
 * Accounts) are addressing, and what one paired engine's connection looks
 * like as a row. The desktop has no analog — its settings address the
 * implicit local engine by construction (`remote_access.rs:37-39`); the
 * web's closest concept is `fleet.active`, surfaced and switched by these
 * rows.
 */

/**
 * The engine those settings pages address, named only when the fleet is
 * plural: `engineHost(fleet.active)` with two or more engines paired,
 * else null — the single-engine case is unambiguous and shows no
 * indicator at all.
 */
export function settingsEngineLabel(fleet: FleetState): string | null {
  return fleet.engines.length > 1 && fleet.active !== null ? engineHost(fleet.active) : null;
}

/** One paired engine's connection view off its registry entry state. */
export interface EngineConnectionView {
  /** The shared status-dot class (`dot-connected` and siblings). */
  readonly dot: string;
  /** The connection label for the row's meta line. */
  readonly label: string;
  /** True when the entry is parked — the row offers "Pair again". */
  readonly pairable: boolean;
}

/**
 * The engine-row state mapping: "Starting…" while the registry entry is
 * still pending, "Connected"/"Reconnecting…" off its live state, and parked
 * reads "Engine changed" when the off-reason names identity, "Session
 * revoked" otherwise — parked alone is pairable.
 */
export function engineConnection(entry: EngineEntrySnapshot | null): EngineConnectionView {
  if (entry === null) {
    return { dot: "dot-connecting", label: "Starting…", pairable: false };
  }
  switch (entry.state) {
    case "connected":
      return { dot: "dot-connected", label: "Connected", pairable: false };
    case "reconnecting":
      return { dot: "dot-reconnecting", label: "Reconnecting…", pairable: false };
    case "off":
      return {
        dot: "dot-parked",
        label: (entry.lastError ?? "").includes("identity") ? "Engine changed" : "Session revoked",
        pairable: true,
      };
  }
}
