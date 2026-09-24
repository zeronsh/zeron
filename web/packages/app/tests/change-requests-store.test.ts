import { describe, expect, it, vi } from "vitest";
import type { ChangeRequestSummary, CheckoutChangeRequestStatus } from "@zeron/proto";
import { RpcError, type WatchHandle } from "@zeron/engine-client";
import {
  ChangeRequestStore,
  checkoutKey,
  providerForCheckout,
  watchParams,
  type ChangeRequestTarget,
} from "../src/state/change-requests-store";

type WatchHandlers = {
  onItem: (item: unknown, context: { generation: number }) => void;
  onEnd?: (error: RpcError | undefined) => void;
};

interface FakeWatch {
  method: string;
  params: unknown;
  handlers: WatchHandlers;
  cancel: () => void;
}

class FakeClient {
  generation = 1;
  watches: FakeWatch[] = [];

  async call<T>(): Promise<T> {
    return {} as T;
  }

  watch<T>(method: string, params: unknown, handlers: WatchHandlers): WatchHandle {
    const watch: FakeWatch = {
      method,
      params,
      handlers: handlers as WatchHandlers,
      cancel: vi.fn(),
    };
    this.watches.push(watch);
    return { method, cancel: () => watch.cancel() };
  }
}

function status(input: Partial<CheckoutChangeRequestStatus>): CheckoutChangeRequestStatus {
  return {
    checkoutId: "checkout-1",
    deviceId: "device-1",
    cwd: "/repo/acme/zeron",
    branch: "feature/pr",
    changeRequest: null,
    updatedAt: "2026-01-01T00:00:00Z",
    ...input,
  };
}

function summary(input: Partial<ChangeRequestSummary>): ChangeRequestSummary {
  return {
    provider: "github",
    number: 90,
    title: "Pull request 90",
    url: "https://github.com/acme/zeron/pull/90",
    state: "open",
    baseRef: "main",
    headRef: "feature/pr",
    ...input,
  };
}

function deliver(watch: FakeWatch, item: unknown, generation = 1): void {
  watch.handlers.onItem(item, { generation });
}

describe("ChangeRequestStore provider tracking", () => {
  it("records the engine-detected provider on the first CR observation", () => {
    const client = new FakeClient();
    const store = new ChangeRequestStore(client);
    store.setTargets([{ deviceId: "device-1", cwd: "/repo/acme/zeron", branch: "feature/pr", checkoutId: null }]);

    deliver(client.watches[0]!, status({ changeRequest: summary({ provider: "gitlab" }) }));

    const snap = store.getSnapshot();
    expect(providerForCheckout(snap.providers, "device-1", "/repo/acme/zeron")).toBe("gitlab");
  });

  it("keeps the last known provider across sibling branches", () => {
    // Once the engine resolves `main` against GitLab, the store should still
    // hand the same provider back when we switch the active target to a
    // sibling branch whose lookup returns `changeRequest: null`.
    const client = new FakeClient();
    const store = new ChangeRequestStore(client);
    store.setTargets([{ deviceId: "device-1", cwd: "/repo/acme/zeron", branch: "main", checkoutId: null }]);
    deliver(client.watches[0]!, status({ branch: "main", changeRequest: summary({ provider: "gitlab", number: 1, headRef: "main" }) }));
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/acme/zeron")).toBe("gitlab");

    store.setTargets([{ deviceId: "device-1", cwd: "/repo/acme/zeron", branch: "feature/pr", checkoutId: null }]);
    const newWatch = client.watches[client.watches.length - 1]!;
    deliver(newWatch, status({ branch: "feature/pr", changeRequest: null }));

    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/acme/zeron")).toBe("gitlab");
  });

  it("returns null when no provider has been observed yet for the checkout", () => {
    const client = new FakeClient();
    const store = new ChangeRequestStore(client);
    store.setTargets([{ deviceId: "device-1", cwd: "/repo/acme/zeron", branch: "feature/pr", checkoutId: null }]);
    deliver(client.watches[0]!, status({ changeRequest: null }));
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/acme/zeron")).toBeNull();
  });

  it("isolates providers by checkout", () => {
    const client = new FakeClient();
    const store = new ChangeRequestStore(client);
    store.setTargets([
      { deviceId: "device-1", cwd: "/repo/a", branch: "feature/x", checkoutId: null },
      { deviceId: "device-1", cwd: "/repo/b", branch: "feature/y", checkoutId: null },
    ]);
    deliver(client.watches[0]!, status({ cwd: "/repo/a", branch: "feature/x", changeRequest: summary({ provider: "github", number: 1 }) }));
    deliver(client.watches[1]!, status({ cwd: "/repo/b", branch: "feature/y", changeRequest: summary({ provider: "bitbucket", number: 2 }) }));
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/a")).toBe("github");
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/b")).toBe("bitbucket");
  });

  it("ignores empty provider strings and still surfaces them as null", () => {
    const client = new FakeClient();
    const store = new ChangeRequestStore(client);
    store.setTargets([{ deviceId: "device-1", cwd: "/repo/a", branch: "feature/x", checkoutId: null }]);
    deliver(client.watches[0]!, status({ changeRequest: summary({ provider: "   " }) }));
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/a")).toBeNull();
  });

  it("clears providers on reset", () => {
    const client = new FakeClient();
    const store = new ChangeRequestStore(client);
    store.setTargets([{ deviceId: "device-1", cwd: "/repo/acme/zeron", branch: "feature/pr", checkoutId: null }]);
    deliver(client.watches[0]!, status({ changeRequest: summary({ provider: "github" }) }));
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/acme/zeron")).toBe("github");

    store.reset();
    expect(providerForCheckout(store.getSnapshot().providers, "device-1", "/repo/acme/zeron")).toBeNull();
  });
});

describe("checkoutKey", () => {
  it("uses a NUL separator so deviceId and cwd cannot collide", () => {
    expect(checkoutKey("a/b", "c")).not.toBe(checkoutKey("a", "b/c"));
    expect(checkoutKey("device-1", "/repo")).toBe("device-1\u0000/repo");
  });
});

describe("watchParams", () => {
  const target: ChangeRequestTarget = { deviceId: "device-1", cwd: "/repo", branch: "feature/pr", checkoutId: null };

  it("omits targetDeviceId when the target is the local device", () => {
    // `watch_params` (change_requests.rs:238-284): the engine resolves its
    // own device without the hop — byte-for-byte desktop parity.
    expect(watchParams(target, "device-1")).toEqual({ cwd: "/repo", branch: "feature/pr" });
  });

  it("rides targetDeviceId for remote targets", () => {
    expect(watchParams(target, "device-2")).toEqual({
      cwd: "/repo",
      branch: "feature/pr",
      targetDeviceId: "device-1",
    });
    expect(watchParams(target, null)).toEqual({
      cwd: "/repo",
      branch: "feature/pr",
      targetDeviceId: "device-1",
    });
  });

  it("re-arms watches when the local device id changes", () => {
    const client = new FakeClient();
    const store = new ChangeRequestStore(client, { localDeviceId: "device-2" });
    store.setTargets([target]);
    expect(client.watches).toHaveLength(1);
    expect(client.watches[0]!.params).toEqual({ cwd: "/repo", branch: "feature/pr", targetDeviceId: "device-1" });

    // Same id again: no re-arm.
    store.setLocalDevice("device-2");
    expect(client.watches).toHaveLength(1);

    // Learn the target IS the local device → the watch re-arms without
    // targetDeviceId.
    store.setLocalDevice("device-1");
    expect(client.watches).toHaveLength(2);
    expect(client.watches[1]!.params).toEqual({ cwd: "/repo", branch: "feature/pr" });
  });
});
