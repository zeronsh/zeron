import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BeginQueueEditOutcome,
  FinishQueueEditOutcome,
  QueuedMessage,
  RenewQueueEditOutcome,
  WatchQueueSnapshot,
} from "@zeron/proto";
import { RpcError, type WatchHandle } from "@zeron/engine-client";
import { QueueStore } from "../src/state/queue-store";

type WatchHandlers = {
  onItem: (item: unknown, context: { generation: number }) => void;
  onEnd?: (error: RpcError | undefined) => void;
};

interface Call {
  method: string;
  params: unknown;
}

interface FakeWatch {
  method: string;
  params: unknown;
  handlers: WatchHandlers;
  cancel: () => void;
}

class FakeClient {
  readonly calls: Call[] = [];
  replies: Map<string, unknown> = new Map();
  nextError: Error | null = null;
  generation = 1;
  watches: FakeWatch[] = [];

  async call<T>(method: string, params?: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (this.nextError !== null) {
      const error = this.nextError;
      this.nextError = null;
      throw error;
    }
    const byMethod = this.replies.get(method);
    return (byMethod ?? {}) as T;
  }

  watch<T>(method: string, params: unknown, handlers: WatchHandlers): WatchHandle {
    const watch: FakeWatch = {
      method,
      params,
      handlers: handlers as WatchHandlers,
      cancel: () => {},
    };
    this.watches.push(watch);
    return { method, cancel: () => watch.cancel() };
  }
}

function row(id: string, overrides: Partial<QueuedMessage> = {}): QueuedMessage {
  return {
    id,
    text: `text-${id}`,
    issuedBy: "device-1",
    issuedAt: 1_700_000_000_000,
    ...overrides,
  };
}

function snapshotFor(rows: QueuedMessage[]): WatchQueueSnapshot {
  return { items: rows };
}

const CHAT = "chat-1";
const DEVICE = "device-1";

describe("QueueStore", () => {
  let client: FakeClient;
  let store: QueueStore;

  beforeEach(() => {
    client = new FakeClient();
    store = new QueueStore(client as unknown as ConstructorParameters<typeof QueueStore>[0], CHAT, { editorDeviceId: DEVICE });
  });

  afterEach(() => {
    store.dispose();
  });

  it("subscribes to WatchQueue with the chat id", () => {
    expect(client.watches).toHaveLength(1);
    expect(client.watches[0]!.method).toBe("WatchQueue");
    expect(client.watches[0]!.params).toEqual({ chatId: CHAT });
  });

  it("starts empty and unloaded", () => {
    const snap = store.getSnapshot();
    expect(snap.rows).toEqual([]);
    expect(snap.loaded).toBe(false);
    expect(snap.error).toBe(null);
    expect(snap.editLease).toBe(null);
  });

  it("applies the first watch item as the snapshot", () => {
    const rows = [row("qm-1"), row("qm-2")];
    client.watches[0]!.handlers.onItem(snapshotFor(rows), { generation: 1 });
    const snap = store.getSnapshot();
    expect(snap.loaded).toBe(true);
    expect(snap.rows.map((r) => r.id)).toEqual(["qm-1", "qm-2"]);
  });

  it("preserves row identity for unchanged rows on a re-delivery", () => {
    const rows = [row("qm-1", { text: "ship" })];
    client.watches[0]!.handlers.onItem(snapshotFor(rows), { generation: 1 });
    const before = store.getSnapshot().rows[0];
    client.watches[0]!.handlers.onItem(snapshotFor(rows), { generation: 1 });
    const after = store.getSnapshot().rows[0];
    expect(after).toBe(before);
  });

  it("adopts a fresh row when text changes", () => {
    client.watches[0]!.handlers.onItem(snapshotFor([row("qm-1", { text: "ship" })]), { generation: 1 });
    const before = store.getSnapshot().rows[0];
    client.watches[0]!.handlers.onItem(snapshotFor([row("qm-1", { text: "ship-it" })]), { generation: 1 });
    const after = store.getSnapshot().rows[0];
    expect(after).not.toBe(before);
    expect(after?.text).toBe("ship-it");
  });

  it("records the stream error and keeps the last rows", () => {
    client.watches[0]!.handlers.onItem(snapshotFor([row("qm-1")]), { generation: 1 });
    client.watches[0]!.handlers.onEnd?.(new RpcError("transport", "socket closed"));
    const snap = store.getSnapshot();
    expect(snap.error).toBe("socket closed");
    expect(snap.rows).toHaveLength(1);
  });

  it("drops malformed watch items", () => {
    const log = vi.fn();
    store.dispose();
    client = new FakeClient();
    store = new QueueStore(client as unknown as ConstructorParameters<typeof QueueStore>[0], CHAT, { editorDeviceId: DEVICE, log });
    client.watches[0]!.handlers.onItem({ wrong: true }, { generation: 1 });
    client.watches[0]!.handlers.onItem(null, { generation: 1 });
    client.watches[0]!.handlers.onItem("not-an-object", { generation: 1 });
    expect(store.getSnapshot().rows).toEqual([]);
    expect(log).toHaveBeenCalled();
  });

  it("resubscribes on resubscribe() and clears state", () => {
    client.watches[0]!.handlers.onItem(snapshotFor([row("qm-1")]), { generation: 1 });
    expect(client.watches).toHaveLength(1);
    store.resubscribe();
    expect(client.watches).toHaveLength(2);
    expect(client.watches[1]!.method).toBe("WatchQueue");
    expect(store.getSnapshot().rows).toEqual([]);
    expect(store.getSnapshot().loaded).toBe(false);
  });

  describe("bound mutations", () => {
    it("sendNow sends SendQueuedMessageNow with chatId + id and returns the ack", async () => {
      client.replies.set("SendQueuedMessageNow", { sent: true });
      expect(await store.sendNow("qm-1")).toBe(true);
      expect(client.calls).toEqual([{ method: "SendQueuedMessageNow", params: { chatId: CHAT, id: "qm-1" } }]);
    });

    it("steerNow mirrors sendNow with SteerQueuedMessageNow", async () => {
      client.replies.set("SteerQueuedMessageNow", { sent: true });
      expect(await store.steerNow("qm-1")).toBe(true);
      expect(client.calls[0]!.params).toEqual({ chatId: CHAT, id: "qm-1" });
    });

    it("move sends the toIndex and reports the ack", async () => {
      client.replies.set("MoveQueuedMessage", { changed: true });
      expect(await store.move("qm-1", 3)).toBe(true);
      expect(client.calls[0]!.params).toEqual({ chatId: CHAT, id: "qm-1", toIndex: 3 });
    });

    it("remove sends the row id and reports the ack", async () => {
      client.replies.set("RemoveQueuedMessage", { removed: true });
      expect(await store.remove("qm-1")).toBe(true);
      expect(client.calls[0]!.params).toEqual({ chatId: CHAT, id: "qm-1" });
    });
  });

  describe("edit lease", () => {
    it("beginEdit records the acquired lease on the snapshot", async () => {
      const acquired: BeginQueueEditOutcome = {
        outcome: "acquired",
        leaseId: "lease-1",
        text: "snapshot text",
        attachments: [],
        baseTextHash: "hash-1",
        expiresAtMs: 1_700_000_060_000,
      };
      client.replies.set("BeginQueuedMessageEdit", acquired);
      const outcome = await store.beginEdit("qm-1", "instance-1");
      expect(outcome).toEqual({ kind: "acquired", leaseId: "lease-1", text: "snapshot text", attachments: [], baseTextHash: "hash-1", expiresAtMs: 1_700_000_060_000 });
      const lease = store.getSnapshot().editLease;
      expect(lease).not.toBe(null);
      expect(lease?.leaseId).toBe("lease-1");
      expect(lease?.messageId).toBe("qm-1");
      expect(lease?.baseText).toBe("snapshot text");
    });

    it("beginEdit does not record a lease when the outcome is locked", async () => {
      client.replies.set("BeginQueuedMessageEdit", { outcome: "locked", ownerDeviceId: "device-2", expiresAtMs: 1 });
      const outcome = await store.beginEdit("qm-1", "instance-1");
      expect(outcome.kind).toBe("locked");
      expect(store.getSnapshot().editLease).toBe(null);
    });

    it("renewEdit updates the lease expiry on renewed and clears it on lost", async () => {
      client.replies.set("BeginQueuedMessageEdit", {
        outcome: "acquired",
        leaseId: "lease-1",
        text: "x",
        attachments: [],
        baseTextHash: "h",
        expiresAtMs: 1,
      });
      await store.beginEdit("qm-1", "instance-1");
      client.replies.set("RenewQueuedMessageEdit", { outcome: "renewed", expiresAtMs: 9_999 } as RenewQueueEditOutcome);
      const renewed = await store.renewEdit();
      expect(renewed).toEqual({ kind: "renewed", expiresAtMs: 9_999 });
      expect(store.getSnapshot().editLease?.expiresAtMs).toBe(9_999);

      client.replies.set("RenewQueuedMessageEdit", { outcome: "lost" });
      const lost = await store.renewEdit();
      expect(lost).toEqual({ kind: "lost" });
      expect(store.getSnapshot().editLease).toBe(null);
    });

    it("finishEdit commit includes the lease hash + new text", async () => {
      client.replies.set("BeginQueuedMessageEdit", {
        outcome: "acquired",
        leaseId: "lease-1",
        text: "old",
        attachments: [],
        baseTextHash: "hash-1",
        expiresAtMs: 1,
      });
      await store.beginEdit("qm-1", "instance-1");
      client.replies.set("FinishQueuedMessageEdit", { outcome: "committed" } as FinishQueueEditOutcome);
      const outcome = await store.finishEdit("commit", { text: "new" });
      expect(outcome).toEqual({ kind: "committed" });
      expect(client.calls.at(-1)!.params).toEqual({
        chatId: CHAT,
        id: "qm-1",
        leaseId: "lease-1",
        action: "commit",
        text: "new",
        expectedTextHash: "hash-1",
      });
      expect(store.getSnapshot().editLease).toBe(null);
    });

    it("finishEdit cancel never sends text/expectedTextHash", async () => {
      client.replies.set("BeginQueuedMessageEdit", {
        outcome: "acquired",
        leaseId: "lease-1",
        text: "old",
        attachments: [],
        baseTextHash: "hash-1",
        expiresAtMs: 1,
      });
      await store.beginEdit("qm-1", "instance-1");
      client.replies.set("FinishQueuedMessageEdit", { outcome: "cancelled" } as FinishQueueEditOutcome);
      await store.finishEdit("cancel");
      const params = client.calls.at(-1)!.params as Record<string, unknown>;
      expect(params["action"]).toBe("cancel");
      expect(params).not.toHaveProperty("text");
      expect(params).not.toHaveProperty("expectedTextHash");
    });

    it("finishEdit missing when no lease is held", async () => {
      const outcome = await store.finishEdit("cancel");
      expect(outcome).toEqual({ kind: "missing" });
      expect(client.calls).toHaveLength(0);
    });
  });

  it("isOurGate returns true only when the host gate's lease id matches our local lease", () => {
    client.watches[0]!.handlers.onItem(
      snapshotFor([
        row("qm-1", {
          deliveryGate: {
            kind: "editing",
            leaseId: "lease-1",
            ownerDeviceId: DEVICE,
            ownerInstanceId: "instance-1",
            acquiredAtMs: 0,
            expiresAtMs: 9_999_999,
            baseTextHash: "h",
          },
        }),
      ]),
      { generation: 1 },
    );
    // No lease acquired locally yet → false.
    expect(store.isOurGate("qm-1")).toBe(false);
  });
});