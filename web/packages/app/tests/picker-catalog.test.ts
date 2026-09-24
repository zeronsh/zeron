import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RpcError } from "@zeron/engine-client";
import type { HarnessDescriptor, Model } from "@zeron/proto";
import { PickerCatalog } from "../src/state/picker-catalog";
import { HARNESS_IN_FLIGHT_MS } from "../src/lib/catalog-loading";

interface Call {
  method: string;
  params: unknown;
}

class FakeClient {
  readonly calls: Call[] = [];
  harnesses: HarnessDescriptor[] = [];
  models: Model[] = [];
  nextError: Error | null = null;
  /**
   * Test seam: hang `ListHarnesses` — each hung call's deferred lands in
   * `harnessDeferreds` for the test to settle (a wedged first-connect
   * call the unary timeout would otherwise own for 30s).
   */
  deferHarnesses = false;
  readonly harnessDeferreds: Array<{ resolve: (rows: HarnessDescriptor[]) => void; reject: (error: Error) => void }> = [];
  readonly #statusListeners = new Set<(status: { state: string }) => void>();

  onStatus(listener: (status: { state: string }) => void): () => void {
    this.#statusListeners.add(listener);
    return () => this.#statusListeners.delete(listener);
  }

  /** Test seam: any engine status change. */
  emitStatus(state: string): void {
    for (const listener of this.#statusListeners) {
      listener({ state });
    }
  }

  /** Test seam: the engine finished dialing. */
  emitConnected(): void {
    this.emitStatus("connected");
  }

  async call<T>(method: string, params?: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (method === "ListHarnesses" && this.deferHarnesses) {
      return new Promise<T>((resolve, reject) => {
        this.harnessDeferreds.push({
          resolve: (rows) => resolve(rows as unknown as T),
          reject,
        });
      });
    }
    if (this.nextError !== null) {
      const error = this.nextError;
      this.nextError = null;
      throw error;
    }
    if (method === "ListHarnesses") {
      return this.harnesses as unknown as T;
    }
    if (method === "ListModels") {
      return this.models as unknown as T;
    }
    return {} as T;
  }
}

describe("PickerCatalog", () => {
  let client: FakeClient;
  let catalog: PickerCatalog;

  beforeEach(() => {
    client = new FakeClient();
    catalog = new PickerCatalog(client as unknown as ConstructorParameters<typeof PickerCatalog>[0]);
  });

  afterEach(() => {
    catalog.dispose();
  });

  it("starts unloaded", () => {
    const harnesses = catalog.getHarnesses();
    expect(harnesses.loaded).toBe(false);
    expect(harnesses.loading).toBe(false);
    expect(harnesses.rows).toEqual([]);
  });

  it("loadHarnesses fetches once and caches the rows", async () => {
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    await catalog.loadHarnesses();
    expect(client.calls).toEqual([{ method: "ListHarnesses", params: {} }]);
    expect(catalog.getHarnesses().rows).toHaveLength(1);
    expect(catalog.getHarnesses().loaded).toBe(true);

    // A second call is a no-op until invalidate.
    await catalog.loadHarnesses();
    expect(client.calls).toHaveLength(1);
  });

  it("loadModels fetches per harness and caches by harness id", async () => {
    client.models = [{ id: "sonnet", label: "Sonnet", reasoningLevels: ["medium"], options: [] }];
    await catalog.loadModels("claude-code");
    expect(client.calls).toEqual([{ method: "ListModels", params: { harness: "claude-code" } }]);
    expect(catalog.getModels("claude-code").rows).toHaveLength(1);
    expect(catalog.getModels("codex").rows).toEqual([]);
  });

  it("surfaces engine failures as a per-collection error", async () => {
    client.nextError = new RpcError("transport", "engine offline");
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().error).toBe("engine offline");
  });

  it("degrades to an empty list when the engine lacks the method", async () => {
    client.nextError = new RpcError("unknown-method", "unknown method: ListHarnesses");
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().rows).toEqual([]);
    expect(catalog.getHarnesses().loaded).toBe(true);
  });

  it("notifies subscribers on harness refresh", async () => {
    const listener = vi.fn();
    catalog.subscribe(listener);
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    await catalog.loadHarnesses();
    expect(listener).toHaveBeenCalled();
  });

  it("invalidate clears the catalog so the next fetch re-loads", async () => {
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().rows).toHaveLength(1);
    catalog.invalidate();
    expect(catalog.getHarnesses().rows).toEqual([]);
    expect(catalog.getHarnesses().loaded).toBe(false);
    await catalog.loadHarnesses();
    expect(client.calls.filter((call) => call.method === "ListHarnesses")).toHaveLength(2);
  });

  it("a forced refresh reloads a loaded slot without clearing its rows", async () => {
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    await catalog.loadHarnesses();
    const stale = catalog.getHarnesses().rows;
    // The forced load fires a second call while the old rows stay visible…
    const refresh = catalog.loadHarnesses({ force: true });
    expect(client.calls.filter((call) => call.method === "ListHarnesses")).toHaveLength(2);
    expect(catalog.getHarnesses().rows).toBe(stale);
    await refresh;
    // …and the fresh catalog replaces them.
    expect(catalog.getHarnesses().loaded).toBe(true);
  });

  it("rides targetDeviceId when the catalog targets another device", async () => {
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    catalog.setTargetDevice("remote-device");
    await catalog.loadHarnesses();
    const call = client.calls.find((entry) => entry.method === "ListHarnesses");
    expect(call?.params).toEqual({ targetDeviceId: "remote-device" });
  });

  it("setTargetDevice invalidates and re-kicks the harness catalog", async () => {
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    await catalog.loadHarnesses();
    catalog.setTargetDevice("remote-device");
    expect(catalog.getHarnesses().loaded).toBe(false);
    const calls = client.calls.filter((entry) => entry.method === "ListHarnesses");
    expect(calls).toHaveLength(2);
    expect((calls[1]!.params as { targetDeviceId?: string }).targetDeviceId).toBe("remote-device");
  });

  it("an offline call retries once the engine connects (the reload race)", async () => {
    // A page-load call races the websocket dial and fails immediately.
    client.nextError = new RpcError("transport", "Engine is offline; reconnecting");
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().error).toBe("Engine is offline; reconnecting");
    expect(catalog.getHarnesses().loaded).toBe(false);
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    client.emitConnected();
    // The connected status re-kicked the errored slot.
    await Promise.resolve();
    for (let attempt = 0; attempt < 10 && !catalog.getHarnesses().loaded; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(catalog.getHarnesses().loaded).toBe(true);
    expect(catalog.getHarnesses().rows).toHaveLength(1);
  });

  it("an offline error re-arms on any status change, not only connected", async () => {
    // The boot race's blind spot: the cache-seeded mount fires the load
    // while the client is pre-dial, and the dial's "connected" can land
    // BEFORE the rejection processes — leaving no future event to re-kick
    // the slot. Any later status change must re-arm it.
    client.nextError = new RpcError("transport", "Engine is offline; reconnecting");
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().error).toBe("Engine is offline; reconnecting");
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    client.emitStatus("connecting");
    await Promise.resolve();
    for (let attempt = 0; attempt < 10 && !catalog.getHarnesses().loaded; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(catalog.getHarnesses().loaded).toBe(true);
    expect(catalog.getHarnesses().rows).toHaveLength(1);
  });

  it("a non-offline error is not re-armed by a non-connected status change", async () => {
    // A failure that landed while the connection was up (the unary call
    // timeout) heals only through "connected", the focus cadence, or the
    // card-open force — a mid-reconnect status change must not re-kick it.
    client.nextError = new RpcError("timeout", "call timed out");
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().error).toBe("call timed out");
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    client.emitStatus("reconnecting");
    await new Promise((resolve) => setTimeout(resolve, 5));
    expect(catalog.getHarnesses().error).toBe("call timed out");
    expect(client.calls.filter((call) => call.method === "ListHarnesses")).toHaveLength(1);
    client.emitConnected();
    await Promise.resolve();
    for (let attempt = 0; attempt < 10 && !catalog.getHarnesses().loaded; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(catalog.getHarnesses().loaded).toBe(true);
  });

  it("a forced open plus the idle cadence fire exactly one ListHarnesses call", async () => {
    // Card open with an Idle slot: the open effect's forced load and the
    // cadence's non-forced kick race in one commit — the in-flight guard
    // keeps them single-flight, so the skeletons never double-load.
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    const forced = catalog.loadHarnesses({ force: true });
    void catalog.loadHarnesses();
    expect(catalog.getHarnesses().loading).toBe(true);
    await forced;
    await catalog.loadHarnesses();
    expect(client.calls.filter((call) => call.method === "ListHarnesses")).toHaveLength(1);
    expect(catalog.getHarnesses().loaded).toBe(true);
  });

  it("a wedged in-flight load past the 10s bound is superseded, not held for the unary timeout (ticket 61 hole 2)", async () => {
    // A first-connect ListHarnesses that goes out but hangs held the
    // slot for the full 30s unary timeout with nothing re-kickable —
    // #harnessesInFlight swallowed every heal attempt. The bound gives
    // the flight a lifetime: past it, a forced kick (the card-open force,
    // the heal) supersedes the flight and its late landing is dropped.
    vi.useFakeTimers();
    try {
      client.deferHarnesses = true;
      const wedged = catalog.loadHarnesses();
      expect(catalog.getHarnesses().loading).toBe(true);
      // A young flight owns the slot: the forced re-kick is swallowed.
      await catalog.loadHarnesses({ force: true });
      expect(client.harnessDeferreds).toHaveLength(1);
      vi.advanceTimersByTime(HARNESS_IN_FLIGHT_MS + 1);
      const rekick = catalog.loadHarnesses({ force: true });
      expect(client.harnessDeferreds).toHaveLength(2);
      client.harnessDeferreds[1]!.resolve([
        { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
      ]);
      await rekick;
      expect(catalog.getHarnesses().loaded).toBe(true);
      expect(catalog.getHarnesses().rows).toHaveLength(1);
      // The wedged flight settles late and is dropped — but only while it
      // does not own a token a later flight reused. Start a fresh forced
      // kick (the zombie still pending), land the zombie mid-flight, and
      // confirm it cannot clobber the slot the fresh flight owns.
      const third = catalog.loadHarnesses({ force: true });
      expect(client.harnessDeferreds).toHaveLength(3);
      client.harnessDeferreds[0]!.resolve([
        { id: "codex", name: "Codex", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
      ]);
      await wedged;
      expect(catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
      client.harnessDeferreds[2]!.resolve([
        { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
      ]);
      await third;
      expect(catalog.getHarnesses().rows.map((row) => row.id)).toEqual(["claude-code"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("an error latched mid-reconnect re-arms on the next dial's connecting status (ticket 61 hole 3)", async () => {
    // The drop's own "reconnecting" fires BEFORE the teardown rejection
    // latches (client #onClose tears down, then emits), so nothing re-arms
    // the slot at the drop itself. Every dial emits "connecting" now
    // (client #dial, ticket 61 hole 3) — that emission is the heal's next
    // event, and the lattice never waits for a connected that may be
    // seconds away.
    client.emitStatus("reconnecting"); // the drop — nothing errored yet
    client.nextError = new RpcError("closed", "Engine connection closed (1006)");
    await catalog.loadHarnesses(); // the teardown rejection lands
    expect(catalog.getHarnesses().error).toBe("Engine connection closed (1006)");
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    client.emitStatus("connecting"); // dial 2 starts
    await Promise.resolve();
    for (let attempt = 0; attempt < 10 && !catalog.getHarnesses().loaded; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(catalog.getHarnesses().loaded).toBe(true);
    expect(catalog.getHarnesses().rows).toHaveLength(1);
  });

  it("the offline re-arm keys on the error's state, not the literal offline message (ticket 61 hole 4)", async () => {
    // A mid-call teardown message never matched the literal pre-dial
    // string, so the slot latched permanently Error with no retry path
    // until a focus or a re-open. The typed check re-arms every
    // connection-level kind on the next non-connected status change.
    client.nextError = new RpcError("closed", "Engine connection closed (1011)");
    await catalog.loadHarnesses();
    expect(catalog.getHarnesses().error).toBe("Engine connection closed (1011)");
    client.harnesses = [
      { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true },
    ];
    client.emitStatus("reconnecting");
    await Promise.resolve();
    for (let attempt = 0; attempt < 10 && !catalog.getHarnesses().loaded; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    expect(catalog.getHarnesses().loaded).toBe(true);
    expect(catalog.getHarnesses().rows).toHaveLength(1);
  });

  it("normalizes model rows as they land", async () => {
    client.models = [{ id: "titan[1m]", label: "Titan (1M context)", reasoningLevels: [], options: [] }];
    await catalog.loadModels("codex");
    const rows = catalog.getModels("codex").rows;
    expect(rows).toHaveLength(1);
    expect(rows[0]!.id).toBe("titan");
    expect(rows[0]!.label).toBe("Titan");
    expect(rows[0]!.options.some((option) => option.id === "contextWindow")).toBe(true);
  });
});