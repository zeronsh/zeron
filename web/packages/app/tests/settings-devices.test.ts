// @vitest-environment jsdom

/**
 * The Devices page must render ONE row per engine. The fleet's engine list
 * (the legacy pairing-era "Engines" card: dot + id label) and the engine's
 * own device registry describe the same fleet under the WorkOS
 * browser-session model, so rendering both double-listed every engine —
 * the page showed the bare engine row (the device id as its title) above
 * the real device row (tile, name, meta, "This device", Rename). The
 * device rows are the registry of record; the engines card is gone.
 *
 * The REAL page mounts (the mounted-suite idiom: jsdom, act, Base UI and
 * the real Icon components run). The fleet/session/snapshot layers are
 * doubled narrowly — exactly one engine over exactly one device, the
 * dev.embedez.com shape. No JSX (createElement), per-file jsdom pragma.
 */

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DevicesSettingsPage } from "../src/routes/settings-devices";

// ── Controllable doubles ──────────────────────────────────────────────────

const NOW = 1_800_000_000_000;

const h = vi.hoisted(() => {
  const deviceId = "9addd95b-827a-4acd-a050-681e53335811";

  /** The fleet's engine list — the same engine the device row carries. */
  const engines = [
    { baseUrl: "dev-a", credential: "", label: deviceId, sessionId: "owner", pairedAt: 0, deviceId },
  ];

  /** The registry entries the (removed) engines card read. */
  const registry = { engines: [{ key: "dev-a", state: "connected", lastError: null }] };

  /** The active engine's session — its own device is the row's engine. */
  const session = {
    client: {
      engineInfo: { deviceId },
      call: async () => ({}),
    },
  };

  /** The live connection state of the active engine. */
  const status = { state: "connected" };

  /** The engine's device registry — the row of record. */
  const snapshot = {
    devices: {
      rows: [
        {
          id: deviceId,
          name: "threaderipper-server-nvme",
          platform: "linux",
          version: "0.2.84",
          lastSeenAt: new Date(1_800_000_000_000).toISOString(),
          createdAt: new Date(1_800_000_000_000 - 11 * 86_400_000).toISOString(),
        },
      ],
    },
  };

  const navigateCalls: Array<{ to: string }> = [];

  return { engines, registry, session, status, snapshot, navigateCalls };
});

vi.mock("../src/state/fleet", () => ({
  useFleet: () => ({
    active: "dev-a",
    engines: h.engines,
    session: { authenticated: true },
    configurationError: null,
  }),
  useFleetRegistry: () => h.registry,
}));

vi.mock("../src/state/session-provider", () => ({
  useEngineSession: () => h.session,
}));

vi.mock("../src/state/hooks", () => ({
  useEngineStatus: () => h.status,
  useWatchSnapshot: () => h.snapshot,
  useNow: () => NOW,
}));

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => (options: { to: string }): Promise<void> => {
    h.navigateCalls.push({ to: options.to });
    return Promise.resolve();
  },
}));

// ── jsdom gaps the mounted page hits ──────────────────────────────────────

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
  if (typeof Element.prototype.scrollIntoView !== "function") {
    Element.prototype.scrollIntoView = () => {};
  }
  if (typeof globalThis.requestAnimationFrame !== "function") {
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      callback(0);
      return 0;
    }) as typeof requestAnimationFrame;
  }
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

// ── The mounted page ──────────────────────────────────────────────────────

const mounted: Array<() => void> = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!();
  }
});

function mountPage(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root: Root | null = createRoot(container);
  mounted.push(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    container.remove();
  });
  act(() => {
    root!.render(createElement(DevicesSettingsPage));
  });
  return container;
}

describe("DevicesSettingsPage — one row per engine", () => {
  it("renders the device row of record, not a second engine row above it", () => {
    const page = mountPage();

    // The bug: the legacy engines card listed the engine again above the
    // device row (two .settings-row for one engine).
    const rows = page.querySelectorAll(".settings-row");
    expect(rows.length).toBe(1);

    // The one row is the device row — the tile, the engine's name, the
    // "This device" badge of the local engine.
    const row = rows[0]!;
    expect(row.classList.contains("device-row")).toBe(true);
    expect(row.querySelector(".settings-row-title")!.textContent).toBe("threaderipper-server-nvme");
    expect(row.querySelector(".badge")!.textContent).toBe("This device");
  });
});
