import { describe, expect, it } from "vitest";
import {
  canonicalBaseUrl,
  engineHost,
  engineWsEndpoint,
  EngineStore,
  webDeviceLabel,
  type SignInInput,
  type StorageLike,
} from "../src/lib/engine-store";

function memoryStorage(): StorageLike & { dump(): Map<string, string> } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => (map.get(key) ?? null),
    setItem: (key, value) => void map.set(key, value),
    removeItem: (key) => void map.delete(key),
    dump: () => map,
  };
}

function signIn(baseUrl: string, credential: string): SignInInput {
  // The sign-in flow (dev user id or WorkOS access token) has already
  // produced the credential by the time the store sees it.
  return { baseUrl, credential, label: "Web on Windows", sessionId: `s-${credential.slice(0, 3)}` };
}

const HOST_A = "127.0.0.1:27699";
const HOST_B = "192.168.1.20:27699";

describe("EngineStore", () => {
  it("stores a signed-in engine as the active entry", async () => {
    const store = new EngineStore({ storage: memoryStorage(), now: () => 1000 });
    const engine = await store.signInEngine(signIn(`http://${HOST_A}`, "cred-a"));
    expect(engine.baseUrl).toBe(`http://${HOST_A}`);
    expect(engine.credential).toBe("cred-a");
    const state = store.getSnapshot();
    expect(state.active).toBe(`http://${HOST_A}`);
    expect(state.engines).toHaveLength(1);
    expect(store.activeEngine()?.credential).toBe("cred-a");
  });

  it("replaces the credential when the same origin is signed in to again", async () => {
    const store = new EngineStore({ storage: memoryStorage() });
    await store.signInEngine(signIn(`http://${HOST_A}`, "cred-1"));
    await store.signInEngine(signIn(`http://${HOST_A}`, "cred-2"));
    const state = store.getSnapshot();
    expect(state.engines).toHaveLength(1);
    expect(state.engines[0]!.credential).toBe("cred-2");
  });

  it("keeps several engines and switches the active one", async () => {
    const store = new EngineStore({ storage: memoryStorage() });
    await store.signInEngine(signIn(`http://${HOST_A}`, "ca"));
    await store.signInEngine(signIn(`http://${HOST_B}`, "cb"));
    expect(store.getSnapshot().active).toBe(`http://${HOST_B}`);
    store.setActive(`http://${HOST_A}`);
    expect(store.activeEngine()?.credential).toBe("ca");
    store.setActive("http://unknown.example");
    expect(store.getSnapshot().active).toBe(`http://${HOST_A}`);
  });

  it("falls back to the first engine when the active one is removed", async () => {
    const store = new EngineStore({ storage: memoryStorage() });
    await store.signInEngine(signIn(`http://${HOST_A}`, "ca"));
    await store.signInEngine(signIn(`http://${HOST_B}`, "cb"));
    store.setActive(`http://${HOST_B}`);
    store.remove(`http://${HOST_B}`);
    expect(store.getSnapshot().engines).toHaveLength(1);
    expect(store.getSnapshot().active).toBe(`http://${HOST_A}`);
    store.remove(`http://${HOST_A}`);
    expect(store.getSnapshot().engines).toHaveLength(0);
    expect(store.getSnapshot().active).toBe(null);
    expect(store.activeEngine()).toBe(null);
  });

  it("pins the verified device identity and survives reload through storage", async () => {
    const storage = memoryStorage();
    const first = new EngineStore({ storage });
    await first.signInEngine(signIn(`http://${HOST_A}`, "ca"));
    first.pinDevice(`http://${HOST_A}`, "device-xyz");
    const second = new EngineStore({ storage });
    expect(second.activeEngine()?.deviceId).toBe("device-xyz");
    expect(second.activeEngine()?.credential).toBe("ca");
  });

  it("yields a clean state when site data is cleared", async () => {
    const storage = memoryStorage();
    const store = new EngineStore({ storage });
    await store.signInEngine(signIn(`http://${HOST_A}`, "ca"));
    storage.dump().clear();
    const fresh = new EngineStore({ storage });
    expect(fresh.getSnapshot().engines).toHaveLength(0);
    expect(fresh.getSnapshot().active).toBe(null);
  });

  it("preserves damaged persisted bytes and blocks sign-in until repaired", async () => {
    const storage = memoryStorage();
    storage.setItem("zeron.fleet.v1", "{not json");
    const damaged = new EngineStore({ storage });
    // Ticket 31: a damaged-but-present value is NEVER overwritten with a
    // blank one — the store reads empty, surfaces the error, and refuses
    // sign-in until the bytes are repaired.
    expect(damaged.getSnapshot().engines).toHaveLength(0);
    expect(damaged.getSnapshot().configurationError).not.toBe(null);
    expect(storage.getItem("zeron.fleet.v1")).toBe("{not json");
    await expect(damaged.signInEngine(signIn(`http://${HOST_A}`, "ca"))).rejects.toThrow();
    expect(storage.getItem("zeron.fleet.v1")).toBe("{not json");
    // A wrong-version payload is damaged the same way.
    storage.setItem("zeron.fleet.v1", JSON.stringify({ version: 99, active: null, engines: [{}] }));
    const wrongVersion = new EngineStore({ storage });
    expect(wrongVersion.getSnapshot().engines).toHaveLength(0);
    expect(wrongVersion.getSnapshot().configurationError).not.toBe(null);
    expect(JSON.parse(storage.getItem("zeron.fleet.v1")!).version).toBe(99);
  });

  it("ignores persisted entries with a missing active engine", () => {
    const storage = memoryStorage();
    storage.setItem(
      "zeron.fleet.v1",
      JSON.stringify({
        version: 1,
        active: "http://gone.example",
        engines: [{ baseUrl: `http://${HOST_A}`, credential: "c", label: "l", sessionId: "s", pairedAt: 1, deviceId: null }],
      }),
    );
    const store = new EngineStore({ storage });
    expect(store.getSnapshot().active).toBe(`http://${HOST_A}`);
  });

  it("notifies subscribers on actual changes only", async () => {
    const store = new EngineStore({ storage: memoryStorage() });
    let fired = 0;
    const unsubscribe = store.subscribe(() => {
      fired += 1;
    });
    await store.signInEngine(signIn(`http://${HOST_A}`, "ca"));
    store.setActive("http://none.example");
    unsubscribe();
    await store.signInEngine(signIn(`http://${HOST_A}`, "ca"));
    expect(fired).toBe(1);
  });
});

describe("endpoint helpers", () => {
  it("derives the WebSocket endpoint at the listener root", () => {
    expect(engineWsEndpoint(`http://${HOST_A}`)).toBe(`ws://${HOST_A}/`);
    expect(engineWsEndpoint("https://engine.example")).toBe("wss://engine.example/");
  });

  it("canonicalizes origins and shows hosts", () => {
    expect(canonicalBaseUrl("http://LocalHost:27699")).toBe("http://localhost:27699");
    expect(engineHost(`http://${HOST_A}`)).toBe(HOST_A);
    expect(engineHost("https://engine.example")).toBe("engine.example");
  });
});

describe("webDeviceLabel", () => {
  it("names the platform the engine's Devices page will show", () => {
    expect(webDeviceLabel({ userAgentData: { platform: "Windows" } })).toBe("Zeron web on Windows");
    expect(webDeviceLabel({ platform: "macOS" })).toBe("Zeron web on macOS");
    expect(webDeviceLabel({})).toBe("Zeron web on this browser");
  });
});
