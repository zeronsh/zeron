// @vitest-environment jsdom

/**
 * Fleet engine identity stability regressions — the post-login "Maximum
 * update depth exceeded" loop. The real `state/fleet` module runs against
 * a doubled `@zeron/engine-client` boundary reporting one signed-in
 * session and one online device: exactly what the browser sees right
 * after a successful WorkOS login.
 *
 * The contract under test: `useFleet().engines` must keep its identity
 * across re-renders and across device polls that report the same devices.
 * The session provider reconciles on that array, and
 * `reconcileEngineSessions` clones every session whose StoredEngine
 * wrapper identity changed (engine-session.ts:112) — an unstable engines
 * array re-ran that effect on every render, cloning sessions forever and
 * taking React down with it.
 */

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { StoredEngine } from "../src/lib/engine-store";

const h = vi.hoisted(() => {
  let deviceCalls = 0;
  const devices = [{ id: "engine-1", name: "Engine one", online: true }];
  return {
    session: { authenticated: true, ownerId: "user_1", csrfToken: "csrf" },
    // A fresh array per call, like a real fetch: identity churn is the
    // hostile input the store must absorb.
    deviceList: (): Array<{ id: string; name: string; online: boolean }> => {
      deviceCalls += 1;
      return devices.map((device) => ({ ...device }));
    },
    deviceCallCount: (): number => deviceCalls,
  };
});

vi.mock("@zeron/engine-client", () => ({
  EngineRegistry: class {
    constructor(_options: unknown) {}
    sync(): void {}
    subscribe(): () => void {
      return () => {};
    }
    getSnapshot(): { engines: never[] } {
      return { engines: [] };
    }
    clientFor(): null {
      return null;
    }
    watchCacheFor(): null {
      return null;
    }
    restart(): void {}
    async shutdown(): Promise<void> {}
  },
  IndexedDbEngineCache: class {},
  RelaySocket: class {},
  relayDeviceUrl: (deviceId: string) => `wss://relay.test/device/${deviceId}/ws`,
  encodeScopedId: (engineKey: string, id: string) => `${engineKey}:${id}`,
  projectRegistrySnapshot: () => ({ chats: [], spaces: [], devices: [], sessions: [] }),
  fetchBrowserSession: async () => h.session,
  fetchBrowserDevices: async () => h.deviceList(),
  startBrowserLogin: async () => "https://workos.example/authorize",
  browserLogout: async () => {},
}));

describe("useFleet engine identity stability", () => {
  let container: HTMLDivElement;
  let root: Root;
  let latest: readonly StoredEngine[] | undefined;

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
    vi.useRealTimers();
  });

  test("engines keep identity across re-renders and unchanged device polls", async () => {
    vi.useFakeTimers();
    // Import under fake timers: the module boots the fleet at load, and the
    // 10s device poll interval must register on the fake clock.
    const { useFleet } = await import("../src/state/fleet");

    const Probe = ({ tick }: { tick: number }) => {
      const fleet = useFleet();
      latest = fleet.engines;
      return createElement("span", null, tick);
    };
    const render = (tick: number): void => {
      act(() => {
        root.render(createElement(Probe, { tick }));
      });
    };

    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    render(0);
    // Boot: startEdgeFleet resolves the session and the first device list.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    expect(latest).toBeDefined();
    expect(latest).toHaveLength(1);
    expect(latest?.[0]?.deviceId).toBe("engine-1");
    const first = latest!;
    const firstEntry = first[0]!;

    // A plain re-render (parent state change) must not rebuild the array:
    // this is exactly the loop that crashed React after login.
    render(1);
    expect(latest).toBe(first);
    expect(latest?.[0]).toBe(firstEntry);

    // The 10s poll returns a fresh-but-equal list; neither the derived
    // engines nor a subsequent render may churn identity.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(h.deviceCallCount()).toBe(2);
    render(2);
    expect(latest).toBe(first);
    expect(latest?.[0]).toBe(firstEntry);
  });
});
