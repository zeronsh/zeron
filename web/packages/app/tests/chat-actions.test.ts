import { describe, expect, it } from "vitest";
import { encodeScopedId, RpcError } from "@zeron/engine-client";
import type { WatchCacheSnapshot } from "@zeron/engine-client";
import type { Chat, ChatConfig } from "@zeron/proto";
import {
  chatSeenAt,
  createChat,
  deleteChat,
  describeMutateError,
  markChatSeen,
  renameChat,
  resetChatSeen,
  setChatArchived,
  waitForChatRow,
} from "../src/lib/chat-actions";
import { chatPageRow } from "../src/lib/view";

class FakeCaller {
  readonly calls: { method: string; params: unknown }[] = [];
  error: Error | null = null;

  async call<T>(method: string, params?: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (this.error !== null) {
      throw this.error;
    }
    return {} as T;
  }
}

function chat(fields: Partial<Chat>): Chat {
  return {
    id: "chat",
    deviceId: "device-1",
    title: null,
    archived: false,
    cwd: null,
    branch: null,
    checkoutId: null,
    config: null,
    lastMessagePreview: null,
    lastMessageAt: null,
    createdAt: "2026-09-16T10:00:00Z",
    ...fields,
  };
}

/** A fake watch cache: rows mutate via push(), listeners fire per push. */
function fakeCache(initial: Chat[] = []) {
  let rows = initial;
  const listeners = new Set<() => void>();
  return {
    getSnapshot() {
      return { chats: { rows } } as unknown as WatchCacheSnapshot;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    push(row: Chat) {
      rows = [...rows, row];
      for (const listener of listeners) {
        listener();
      }
    },
  };
}

describe("createChat", () => {
  it("mints an id and sends the createChat op into the picked space", async () => {
    const caller = new FakeCaller();
    const chatId = await createChat(caller, { spaceId: "space-1", mintId: () => "chat-1" });
    expect(chatId).toBe("chat-1");
    expect(caller.calls).toEqual([{ method: "Mutate", params: { op: "createChat", chatId: "chat-1", spaceId: "space-1" } }]);
  });

  it("targets the device for a project-less chat and never mixes the two", async () => {
    const caller = new FakeCaller();
    await createChat(caller, { deviceId: "device-9", mintId: () => "chat-2" });
    expect(caller.calls[0]!.params).toEqual({ op: "createChat", chatId: "chat-2", deviceId: "device-9" });
    expect(caller.calls[0]!.params).not.toHaveProperty("spaceId");

    const spaced = new FakeCaller();
    await createChat(spaced, { spaceId: "s", deviceId: "ignored", mintId: () => "chat-3" });
    expect(spaced.calls[0]!.params).not.toHaveProperty("deviceId");
  });

  it("propagates engine failures", async () => {
    const caller = new FakeCaller();
    caller.error = new RpcError("transport", "Engine is offline; reconnecting");
    await expect(createChat(caller, { spaceId: "s" })).rejects.toThrow("Engine is offline");
  });

  // §2.2 (composer.rs:6494-6544): the full wire payload — cwd/branch/config
  // inserted only when present, exactly like the desktop's assembly.
  it("carries the resolved config, the checkout plan's branch, and a worktree-reuse cwd", async () => {
    const caller = new FakeCaller();
    const config: ChatConfig = {
      harness: "claude-code",
      model: "claude-3-5-sonnet",
      reasoning: "high",
      sandbox: "workspace-write",
      modelOptions: { effort: "low" },
    };
    await createChat(caller, {
      spaceId: "space-1",
      mintId: () => "chat-9",
      config,
      branch: "feature/x",
      cwd: "/repo/.worktrees/feature-x",
    });
    expect(caller.calls[0]!.params).toEqual({
      op: "createChat",
      chatId: "chat-9",
      spaceId: "space-1",
      cwd: "/repo/.worktrees/feature-x",
      branch: "feature/x",
      config,
    });
  });

  it("never inserts absent fields — a projectless createChat names the host device outright", async () => {
    const caller = new FakeCaller();
    const config: ChatConfig = {
      harness: "claude-code",
      model: null,
      reasoning: null,
      sandbox: "workspace-write",
      modelOptions: {},
    };
    await createChat(caller, { deviceId: "device-9", mintId: () => "chat-10", config });
    expect(caller.calls[0]!.params).toEqual({
      op: "createChat",
      chatId: "chat-10",
      deviceId: "device-9",
      config,
    });
    // The projectless "~" NEVER rides createChat — it lives on the RunRequest
    // (composer.rs:6515-6521 inserts cwd only for the worktree-reuse override).
    expect(JSON.stringify(caller.calls[0]!.params)).not.toContain("~");
  });
});

describe("canvasSendNavigatesUnderScopedId", () => {
  // §2.3: the wire keeps the RAW mint (createChat, waitForChatRow, the
  // session's own watch cache); the route carries the SCOPED form, so the
  // merged fleet rows' scoped ids match chatPageRow's exact compare.
  it("mints raw, creates and waits raw, navigates scoped, and the page row resolves", async () => {
    const engineKey = "http://192.168.1.4:4312";
    const caller = new FakeCaller();
    const cache = fakeCache();
    // The engine's watch echo lands the row under its RAW id.
    cache.push(chat({ id: "raw-1", deviceId: "device-9" }));

    const chatId = await createChat(caller, { deviceId: "device-9", mintId: () => "raw-1" });
    expect(chatId).toBe("raw-1");
    expect((caller.calls[0]!.params as { chatId: string }).chatId).toBe("raw-1");
    await expect(waitForChatRow(cache, chatId)).resolves.toBe(true);

    // The navigation scopes the mint — the same call add-space's optimistic
    // space rows make.
    const scoped = encodeScopedId(engineKey, chatId);
    expect(scoped.startsWith("engine:v1:")).toBe(true);

    // The merged fleet snapshot scopes the engine's raw row; the page's exact
    // compare resolves under the scoped URL id…
    const merged = [chat({ id: scoped, deviceId: "device-9" })];
    expect(chatPageRow(scoped, merged, [], [], 0)?.chat.id).toBe(scoped);
    // …while the raw URL form misses — the pre-ticket-39 not-found page.
    expect(chatPageRow(chatId, merged, [], [], 0)).toBeUndefined();
  });
});

describe("renameChat", () => {
  it("trims and sends the new title", async () => {
    const caller = new FakeCaller();
    const sent = await renameChat(caller, "chat-1", "  Ship it  ");
    expect(sent).toBe(true);
    expect(caller.calls).toEqual([{ method: "Mutate", params: { op: "renameChat", chatId: "chat-1", title: "Ship it" } }]);
  });

  it("is a no-op for an empty title, like the desktop dialog", async () => {
    const caller = new FakeCaller();
    expect(await renameChat(caller, "chat-1", "   ")).toBe(false);
    expect(await renameChat(caller, "chat-1", "")).toBe(false);
    expect(caller.calls).toHaveLength(0);
  });
});

describe("setChatArchived and deleteChat", () => {
  it("send their ops with the chat id", async () => {
    const caller = new FakeCaller();
    await setChatArchived(caller, "chat-1", true);
    await setChatArchived(caller, "chat-2", false);
    await deleteChat(caller, "chat-1");
    expect(caller.calls.map((entry) => entry.params)).toEqual([
      { op: "setChatArchived", chatId: "chat-1", archived: true },
      { op: "setChatArchived", chatId: "chat-2", archived: false },
      { op: "deleteChat", chatId: "chat-1" },
    ]);
  });
});

describe("describeMutateError", () => {
  it("passes the engine's message through and has a fallback", () => {
    expect(describeMutateError(new RpcError("transport", "Engine is offline; reconnecting"))).toBe(
      "Engine is offline; reconnecting",
    );
    expect(describeMutateError("nope")).toBe("The change could not be applied.");
  });
});

describe("waitForChatRow", () => {
  it("resolves immediately when the row is already there", async () => {
    const cache = fakeCache([chat({ id: "chat-1" })]);
    await expect(waitForChatRow(cache, "chat-1")).resolves.toBe(true);
  });

  it("resolves when the row lands on a later watch emission", async () => {
    const cache = fakeCache();
    const pending = waitForChatRow(cache, "chat-1");
    cache.push(chat({ id: "other" }));
    cache.push(chat({ id: "chat-1" }));
    await expect(pending).resolves.toBe(true);
  });

  it("gives up on timeout so the caller still navigates", async () => {
    const cache = fakeCache();
    await expect(waitForChatRow(cache, "chat-1", 20)).resolves.toBe(false);
  });
});

describe("markChatSeen", () => {
  it("stamps locally first, then fire-and-forgets the mutation", () => {
    resetChatSeen();
    const caller = new FakeCaller();
    expect(markChatSeen(caller, "chat-1", 1_000)).toBe(true);
    // The stamp is visible synchronously — it does not wait on the wire.
    expect(chatSeenAt("chat-1")).toBe(1_000);
    expect(caller.calls).toHaveLength(1);
    expect(caller.calls[0]!.params).toEqual({ op: "markChatSeen", chatId: "chat-1" });
  });

  it("is idempotent: a chat already seen at or after now sends nothing", () => {
    resetChatSeen();
    const caller = new FakeCaller();
    markChatSeen(caller, "chat-1", 2_000);
    expect(markChatSeen(caller, "chat-1", 2_000)).toBe(false);
    expect(markChatSeen(caller, "chat-1", 1_500)).toBe(false);
    expect(caller.calls).toHaveLength(1);
    // A later view stamps again.
    expect(markChatSeen(caller, "chat-1", 3_000)).toBe(true);
    expect(chatSeenAt("chat-1")).toBe(3_000);
  });

  it("keeps the optimistic stamp when the mutation fails", async () => {
    resetChatSeen();
    const caller = new FakeCaller();
    caller.error = new RpcError("transport", "offline");
    markChatSeen(caller, "chat-1", 1_000);
    await Promise.resolve();
    expect(chatSeenAt("chat-1")).toBe(1_000);
  });
});
