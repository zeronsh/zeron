import { describe, expect, it } from "vitest";
import type { EngineEntrySnapshot, EngineConnectionState } from "@zeron/engine-client";
import type { FleetState, StoredEngine } from "../src/lib/engine-store";
import { engineConnection, settingsEngineLabel } from "../src/lib/settings-engine";

const HOST_A = "127.0.0.1:27699";
const HOST_B = "192.168.1.20:27699";

function stored(baseUrl: string): StoredEngine {
  return {
    baseUrl,
    credential: "credential",
    label: "Web on Windows",
    sessionId: "session",
    pairedAt: 1,
    deviceId: null,
  };
}

function fleetOf(active: string | null, engines: readonly StoredEngine[]): FleetState {
  return { active, engines, configurationError: null };
}

function entryOf(state: EngineConnectionState, lastError: string | null): EngineEntrySnapshot {
  return {
    key: `http://${HOST_A}`,
    info: null,
    state,
    lastError,
    generation: 1,
    chats: { rows: [], loaded: false, error: null },
    spaces: { rows: [], loaded: false, error: null },
    devices: { rows: [], loaded: false, error: null },
    sessions: { rows: [], loaded: false, error: null },
  };
}

describe("settingsEngineLabel", () => {
  it("settingsEngineLabelNamesActiveEngineOnlyWhenFleetIsPlural", () => {
    const a = stored(`http://${HOST_A}`);
    const b = stored(`http://${HOST_B}`);
    // 0 engines: nothing to address.
    expect(settingsEngineLabel(fleetOf(null, []))).toBe(null);
    // 1 engine: the single-engine case is unambiguous — no indicator.
    expect(settingsEngineLabel(fleetOf(`http://${HOST_A}`, [a]))).toBe(null);
    // 2+ engines: the active engine's host.
    expect(settingsEngineLabel(fleetOf(`http://${HOST_A}`, [a, b]))).toBe(HOST_A);
    expect(settingsEngineLabel(fleetOf(`http://${HOST_B}`, [a, b]))).toBe(HOST_B);
  });
});

describe("engineConnection (the folded engine-drawer row mapping)", () => {
  it("engineConnectionLabelsParkedAndIdentityChanged", () => {
    const cases: readonly [EngineEntrySnapshot | null, string, string, boolean][] = [
      // No registry entry yet (engine just paired, spawn pending).
      [null, "dot-connecting", "Starting…", false],
      [entryOf("connected", null), "dot-connected", "Connected", false],
      [entryOf("reconnecting", null), "dot-reconnecting", "Reconnecting…", false],
      // Parked without identity in the reason: revoked Session.
      [entryOf("off", "handshake refused: 4401"), "dot-parked", "Session revoked", true],
      // Parked with identity in the reason: the engine changed underneath.
      [entryOf("off", "engine identity mismatch"), "dot-parked", "Engine changed", true],
    ];
    for (const [entry, dot, label, pairable] of cases) {
      expect(engineConnection(entry)).toEqual({ dot, label, pairable });
    }
  });
});
