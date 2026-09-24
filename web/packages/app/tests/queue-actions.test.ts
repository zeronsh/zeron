import { describe, expect, it } from "vitest";
import { RpcError } from "@zeron/engine-client";
import type { BeginQueueEditOutcome, FinishQueueEditOutcome, RenewQueueEditOutcome } from "@zeron/proto";
import {
  beginQueuedMessageEdit,
  describeQueueError,
  finishQueuedMessageEdit,
  mintEditorInstanceId,
  moveQueuedMessage,
  queueMessage,
  removeQueuedMessage,
  renewQueuedMessageEdit,
  sendQueuedMessageNow,
  steerQueuedMessageNow,
  updateQueuedMessage,
} from "../src/lib/queue-actions";

class FakeCaller {
  readonly calls: { method: string; params: unknown }[] = [];
  replies: Map<string, unknown> = new Map();
  nextError: Error | null = null;

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
}

describe("queueMessage", () => {
  it("trims and sends QueueMessage with chatId + text", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueMessage", { id: "qm-1" });
    const id = await queueMessage(caller, "chat-1", "  hello  ");
    expect(id).toBe("qm-1");
    expect(caller.calls).toEqual([{ method: "QueueMessage", params: { chatId: "chat-1", text: "hello" } }]);
  });

  it("includes holdForTurnEnd only when true", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueMessage", { id: "qm-1" });
    await queueMessage(caller, "chat-1", "ship", { holdForTurnEnd: true });
    expect(caller.calls[0]!.params).toEqual({ chatId: "chat-1", text: "ship", holdForTurnEnd: true });

    const caller2 = new FakeCaller();
    caller2.replies.set("QueueMessage", { id: "qm-2" });
    await queueMessage(caller2, "chat-1", "ship", { holdForTurnEnd: false });
    expect(caller2.calls[0]!.params).toEqual({ chatId: "chat-1", text: "ship" });
  });

  it("forwards staged attachments only when non-empty", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueMessage", { id: "qm-1" });
    await queueMessage(caller, "chat-1", "see", { attachments: ["/tmp/a.png", "/tmp/b.png"] });
    expect(caller.calls[0]!.params).toEqual({
      chatId: "chat-1",
      text: "see",
      attachments: ["/tmp/a.png", "/tmp/b.png"],
    });

    const caller2 = new FakeCaller();
    caller2.replies.set("QueueMessage", { id: "qm-2" });
    await queueMessage(caller2, "chat-1", "see", { attachments: [] });
    expect((caller2.calls[0]!.params as Record<string, unknown>)).not.toHaveProperty("attachments");
  });

  it("rejects an empty text before touching the wire", async () => {
    const caller = new FakeCaller();
    await expect(queueMessage(caller, "chat-1", "   ")).rejects.toThrow(/empty/);
    expect(caller.calls).toHaveLength(0);
  });

  it("propagates engine failures", async () => {
    const caller = new FakeCaller();
    caller.nextError = new RpcError("transport", "engine offline");
    await expect(queueMessage(caller, "chat-1", "hello")).rejects.toThrow("engine offline");
  });
});

describe("updateQueuedMessage", () => {
  it("sends the new text (empty text is engine's delete-row contract)", async () => {
    const caller = new FakeCaller();
    await updateQueuedMessage(caller, "chat-1", "qm-1", "newer");
    expect(caller.calls).toEqual([{ method: "UpdateQueuedMessage", params: { chatId: "chat-1", id: "qm-1", text: "newer" } }]);
  });
});

describe("moveQueuedMessage", () => {
  it("sends toIndex as the destination slot", async () => {
    const caller = new FakeCaller();
    caller.replies.set("MoveQueuedMessage", { changed: true });
    const changed = await moveQueuedMessage(caller, "chat-1", "qm-1", 2);
    expect(changed).toBe(true);
    expect(caller.calls).toEqual([{ method: "MoveQueuedMessage", params: { chatId: "chat-1", id: "qm-1", toIndex: 2 } }]);
  });

  it("returns false on a {changed: false} ack (racing device)", async () => {
    const caller = new FakeCaller();
    caller.replies.set("MoveQueuedMessage", { changed: false });
    expect(await moveQueuedMessage(caller, "chat-1", "qm-1", 0)).toBe(false);
  });
});

describe("removeQueuedMessage", () => {
  it("sends the row id and reports the removed ack", async () => {
    const caller = new FakeCaller();
    caller.replies.set("RemoveQueuedMessage", { removed: true });
    expect(await removeQueuedMessage(caller, "chat-1", "qm-1")).toBe(true);
    expect(caller.calls).toEqual([{ method: "RemoveQueuedMessage", params: { chatId: "chat-1", id: "qm-1" } }]);
  });

  it("returns false on a {removed: false} ack (racing device)", async () => {
    const caller = new FakeCaller();
    caller.replies.set("RemoveQueuedMessage", { removed: false });
    expect(await removeQueuedMessage(caller, "chat-1", "qm-1")).toBe(false);
  });
});

describe("sendQueuedMessageNow / steerQueuedMessageNow", () => {
  it("send now sends the chat+id and reports the sent ack", async () => {
    const caller = new FakeCaller();
    caller.replies.set("SendQueuedMessageNow", { sent: true });
    expect(await sendQueuedMessageNow(caller, "chat-1", "qm-1")).toBe(true);
    expect(caller.calls).toEqual([{ method: "SendQueuedMessageNow", params: { chatId: "chat-1", id: "qm-1" } }]);
  });

  it("steer now mirrors send now with the SteerQueuedMessageNow method", async () => {
    const caller = new FakeCaller();
    caller.replies.set("SteerQueuedMessageNow", { sent: true });
    expect(await steerQueuedMessageNow(caller, "chat-1", "qm-1")).toBe(true);
    expect(caller.calls).toEqual([{ method: "SteerQueuedMessageNow", params: { chatId: "chat-1", id: "qm-1" } }]);
  });

  it("returns false on a {sent: false} ack (racing device drained it)", async () => {
    const caller = new FakeCaller();
    caller.replies.set("SendQueuedMessageNow", { sent: false });
    expect(await sendQueuedMessageNow(caller, "chat-1", "qm-1")).toBe(false);
  });
});

describe("beginQueuedMessageEdit", () => {
  it("returns the acquired outcome verbatim", async () => {
    const caller = new FakeCaller();
    const acquired: BeginQueueEditOutcome = {
      outcome: "acquired",
      leaseId: "lease-1",
      text: "ship",
      attachments: [],
      baseTextHash: "hash-1",
      expiresAtMs: 9_999_999,
    };
    caller.replies.set("BeginQueuedMessageEdit", acquired);
    const reply = await beginQueuedMessageEdit(caller, "chat-1", "qm-1", "device-1", "instance-1");
    expect(reply).toEqual(acquired);
    expect(caller.calls).toEqual([
      { method: "BeginQueuedMessageEdit", params: { chatId: "chat-1", id: "qm-1", editorDeviceId: "device-1", editorInstanceId: "instance-1" } },
    ]);
  });

  it("forwards the locked outcome so the caller can show 'Editing on device X'", async () => {
    const caller = new FakeCaller();
    const locked: BeginQueueEditOutcome = { outcome: "locked", ownerDeviceId: "device-2", expiresAtMs: 12_345 };
    caller.replies.set("BeginQueuedMessageEdit", locked);
    expect(await beginQueuedMessageEdit(caller, "chat-1", "qm-1", "device-1", "instance-1")).toEqual(locked);
  });

  it("forwards the missing outcome", async () => {
    const caller = new FakeCaller();
    const missing: BeginQueueEditOutcome = { outcome: "missing" };
    caller.replies.set("BeginQueuedMessageEdit", missing);
    expect(await beginQueuedMessageEdit(caller, "chat-1", "qm-1", "device-1", "instance-1")).toEqual(missing);
  });
});

describe("renewQueuedMessageEdit", () => {
  it("returns renewed {expiresAtMs} verbatim", async () => {
    const caller = new FakeCaller();
    const renewed: RenewQueueEditOutcome = { outcome: "renewed", expiresAtMs: 8_888_888 };
    caller.replies.set("RenewQueuedMessageEdit", renewed);
    expect(await renewQueuedMessageEdit(caller, "chat-1", "qm-1", "lease-1")).toEqual(renewed);
    expect(caller.calls).toEqual([{ method: "RenewQueuedMessageEdit", params: { chatId: "chat-1", id: "qm-1", leaseId: "lease-1" } }]);
  });

  it("forwards the lost outcome", async () => {
    const caller = new FakeCaller();
    caller.replies.set("RenewQueuedMessageEdit", { outcome: "lost" });
    expect(await renewQueuedMessageEdit(caller, "chat-1", "qm-1", "lease-1")).toEqual({ outcome: "lost" });
  });
});

describe("finishQueuedMessageEdit", () => {
  it("commit forwards text + explicit expectedTextHash when the caller supplies both", async () => {
    const caller = new FakeCaller();
    const outcome: FinishQueueEditOutcome = { outcome: "committed" };
    caller.replies.set("FinishQueuedMessageEdit", outcome);
    expect(
      await finishQueuedMessageEdit(caller, "chat-1", "qm-1", "lease-1", "commit", {
        text: "patched",
        expectedTextHash: "hash-1",
      }),
    ).toEqual(outcome);
    expect(caller.calls[0]!.params).toEqual({
      chatId: "chat-1",
      id: "qm-1",
      leaseId: "lease-1",
      action: "commit",
      text: "patched",
      expectedTextHash: "hash-1",
    });
  });

  it("omits text + expectedTextHash on cancel/discard/releaseUnchanged", async () => {
    const caller = new FakeCaller();
    caller.replies.set("FinishQueuedMessageEdit", { outcome: "cancelled" });
    await finishQueuedMessageEdit(caller, "chat-1", "qm-1", "lease-1", "cancel");
    const params = caller.calls[0]!.params as Record<string, unknown>;
    expect(params).toEqual({
      chatId: "chat-1",
      id: "qm-1",
      leaseId: "lease-1",
      action: "cancel",
    });
    expect(params).not.toHaveProperty("text");
    expect(params).not.toHaveProperty("expectedTextHash");
  });

  it("forwards staged attachments only when non-empty", async () => {
    const caller = new FakeCaller();
    caller.replies.set("FinishQueuedMessageEdit", { outcome: "committed" });
    await finishQueuedMessageEdit(caller, "chat-1", "qm-1", "lease-1", "commit", {
      text: "patched",
      attachments: ["/tmp/a.png"],
    });
    const params = caller.calls[0]!.params as Record<string, unknown>;
    expect(params["attachments"]).toEqual(["/tmp/a.png"]);
  });

  it("forwards the conflict outcome (drifted row) verbatim", async () => {
    const caller = new FakeCaller();
    const conflict: FinishQueueEditOutcome = { outcome: "conflict", currentText: "remote wins" };
    caller.replies.set("FinishQueuedMessageEdit", conflict);
    expect(await finishQueuedMessageEdit(caller, "chat-1", "qm-1", "lease-1", "commit", { text: "patched" })).toEqual(conflict);
  });
});

describe("mintEditorInstanceId", () => {
  it("returns unique ids", () => {
    const a = mintEditorInstanceId();
    const b = mintEditorInstanceId();
    expect(a).not.toBe(b);
    expect(a.length).toBeGreaterThan(0);
  });
});

describe("describeQueueError", () => {
  it("passes through RpcError messages with a generic fallback", () => {
    expect(describeQueueError(new RpcError("transport", "engine offline"))).toBe("engine offline");
    expect(describeQueueError("nope")).toBe("The change could not be applied.");
  });
});