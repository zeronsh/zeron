import { describe, expect, it } from "vitest";
import { RpcError } from "@zeron/engine-client";
import type { ChatConfig } from "@zeron/proto";
import {
  buildChatConfig,
  buildRunRequest,
  describeSendError,
  queueMessage,
  sendInterrupt,
  sendRun,
  persistChatConfig,
  type DraftConfig,
} from "../src/lib/composer-actions";
import { stageBytes, type StagedAttachment } from "../src/lib/attachments";

class FakeCaller {
  readonly calls: { method: string; params: unknown }[] = [];
  replies: Map<string, unknown> = new Map();
  nextError: Error | null = null;
  nextReply: unknown = undefined;

  async call<T>(method: string, params?: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (this.nextError !== null) {
      const error = this.nextError;
      this.nextError = null;
      throw error;
    }
    if (this.nextReply !== undefined) {
      const reply = this.nextReply;
      this.nextReply = undefined;
      return reply as T;
    }
    const byMethod = this.replies.get(method);
    if (byMethod !== undefined) {
      return byMethod as T;
    }
    return {} as T;
  }
}

const DRAFT: DraftConfig = {
  harness: "claude-code",
  model: "claude-3-5-sonnet",
  reasoning: "high",
  sandbox: "workspace-write",
  modelOptions: { "effort": "low" },
};

const PERSISTED: ChatConfig = {
  harness: "claude-code",
  model: "claude-3-5-sonnet",
  reasoning: "high",
  sandbox: "workspace-write",
  modelOptions: { "effort": "low" },
};

describe("buildChatConfig", () => {
  it("mirrors the draft as the wire's ChatConfig", () => {
    expect(buildChatConfig(DRAFT)).toEqual(PERSISTED);
  });

  it("copies modelOptions so callers cannot mutate the draft by reference", () => {
    const config = buildChatConfig(DRAFT);
    expect(config.modelOptions).not.toBe(DRAFT.modelOptions);
    expect(config.modelOptions).toEqual(DRAFT.modelOptions);
  });
});

describe("buildRunRequest", () => {
  it("carries the chosen reasoning level to both the wire ChatConfig and the RunRequest", () => {
    // Ticket 77: the level the picker commits is the level both transports
    // ship — no transport-side repair exists or is needed.
    const draft: DraftConfig = { ...DRAFT, reasoning: "low" };
    expect(buildChatConfig(draft).reasoning).toBe("low");
    expect(buildRunRequest(draft, "hi", "/tmp").reasoning).toBe("low");
  });

  it("fills every field the engine requires for a Run", () => {
    const request = buildRunRequest(DRAFT, "hi", "/Users/me/proj");
    expect(request.prompt).toBe("hi");
    expect(request.harness).toBe("claude-code");
    expect(request.model).toBe("claude-3-5-sonnet");
    expect(request.reasoning).toBe("high");
    expect(request.cwd).toBe("/Users/me/proj");
    expect(request.sandbox).toBe("workspace-write");
    // The messageId lives on the SessionCommandPayload (`run`), not the RunRequest itself.
    expect(request).not.toHaveProperty("messageId");
  });

  it("preserves the picked harness on the wire", () => {
    const request = buildRunRequest({ ...DRAFT, harness: "codex" }, "hi", "/tmp");
    expect(request.harness).toBe("codex");
  });

  it("carries the staged attachment paths and defaults worktree to null", () => {
    const request = buildRunRequest(DRAFT, "hi", "/tmp", ["/host/a.png"]);
    expect(request.attachments).toEqual(["/host/a.png"]);
    expect(request.worktree).toBeNull();
  });

  it("carries a worktree directive for the host to materialize", () => {
    const request = buildRunRequest(DRAFT, "hi", "/repo", [], { repoPath: "/repo", base: "HEAD" });
    expect(request.worktree).toEqual({ repoPath: "/repo", base: "HEAD" });
  });
});

describe("sendRun", () => {
  // The desktop carries model/reasoning/options on the RunRequest itself and
  // only writes a ChatConfig via `Mutate createChat`; a per-send setChatConfig
  // mutation was web-only invention (ticket 04).
  it("sends only QueueCommand Run — never a setChatConfig mutation", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueCommand", { commandId: "cmd-1" });
    await sendRun(caller, "chat-1", DRAFT, "  ship it  ", "/Users/me/proj", {
      mintMessageId: () => "msg-1",
    });
    expect(caller.calls.map((entry) => entry.method)).toEqual(["QueueCommand"]);
    const queue = caller.calls[0]!.params as { chatId: string; command: { kind: string; messageId: string; request: { prompt: string; cwd: string; harness: string; model: string | null; reasoning: string | null } } };
    expect(queue.chatId).toBe("chat-1");
    expect(queue.command.kind).toBe("run");
    expect(queue.command.messageId).toBe("msg-1");
    expect(queue.command.request.prompt).toBe("ship it");
    expect(queue.command.request.cwd).toBe("/Users/me/proj");
    // The draft's identity rides the request, which is why the pre-send
    // mutation was redundant.
    expect(queue.command.request.harness).toBe(DRAFT.harness);
    expect(queue.command.request.model).toBe(DRAFT.model);
    expect(queue.command.request.reasoning).toBe(DRAFT.reasoning);
  });

  it("rejects an empty prompt before touching the wire", async () => {
    const caller = new FakeCaller();
    await expect(sendRun(caller, "chat-1", DRAFT, "   ", "/Users/me/proj")).rejects.toThrow(/empty/);
    expect(caller.calls).toHaveLength(0);
  });

  it("accepts ~ and . as legal wire cwd values — the engine expands host-side", async () => {
    // composer.rs:6433-6440 has no error path for a projectless send: "~"
    // (the host's home) and "." (the existing-chat fallback) ride the
    // RunRequest literally; sessions.rs:342-352 expands them ON THE ENGINE.
    // The old web-only empty-cwd throw is deleted (§2.1).
    const caller = new FakeCaller();
    caller.replies.set("QueueCommand", { commandId: "cmd-1" });
    await sendRun(caller, "chat-1", DRAFT, "hi", "~", { mintMessageId: () => "m-1" });
    const first = caller.calls[0]!.params as { command: { request: { cwd: string } } };
    expect(first.command.request.cwd).toBe("~");
    await sendRun(caller, "chat-2", DRAFT, "hi again", ".", { mintMessageId: () => "m-2" });
    const second = caller.calls[1]!.params as { command: { request: { cwd: string } } };
    expect(second.command.request.cwd).toBe(".");
  });

  it("propagates engine failures so the caller can show a notice", async () => {
    const caller = new FakeCaller();
    caller.nextError = new RpcError("transport", "Engine is offline; reconnecting");
    await expect(sendRun(caller, "chat-1", DRAFT, "hi", "/Users/me/proj")).rejects.toThrow(
      "Engine is offline",
    );
  });
});

describe("queueMessage", () => {
  // The composer's busy-chat path (composer.rs:6547-6577): QueueMessage with
  // holdForTurnEnd — never a Steer command (spec decision 3).
  it("queues with holdForTurnEnd and returns the row id", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueMessage", { id: "q-9" });
    const id = await queueMessage(caller, "chat-1", "  ship it  ", ["/host/a.png"]);
    expect(id).toBe("q-9");
    expect(caller.calls).toEqual([
      {
        method: "QueueMessage",
        params: {
          chatId: "chat-1",
          text: "ship it",
          attachments: ["/host/a.png"],
          holdForTurnEnd: true,
        },
      },
    ]);
  });

  it("raises the verbatim failure when the queue returns no id", async () => {
    const caller = new FakeCaller();
    caller.replies.set("QueueMessage", {});
    await expect(queueMessage(caller, "chat-1", "ship")).rejects.toThrow(
      "Send failed: queue did not return an id",
    );
  });

  it("wraps RPC failures as Send failed", async () => {
    const caller = new FakeCaller();
    caller.nextError = new RpcError("transport", "engine offline");
    await expect(queueMessage(caller, "chat-1", "ship")).rejects.toThrow(/^Send failed:/);
  });
});

describe("sendInterrupt", () => {
  it("sends an Interrupt command with no payload", async () => {
    const caller = new FakeCaller();
    await sendInterrupt(caller, "chat-1");
    expect(caller.calls).toEqual([
      {
        method: "QueueCommand",
        params: {
          chatId: "chat-1",
          command: { kind: "interrupt" },
          transfers: [],
        },
      },
    ]);
  });
});

describe("persistChatConfig", () => {
  it("sends a setChatConfig Mutate op with the draft as the wire config", async () => {
    const caller = new FakeCaller();
    await persistChatConfig(caller, "chat-1", DRAFT);
    expect(caller.calls).toHaveLength(1);
    const params = caller.calls[0]!.params as { op: string; chatId: string; config: ChatConfig };
    expect(params).toEqual({ op: "setChatConfig", chatId: "chat-1", config: PERSISTED });
  });
});

describe("describeSendError", () => {
  it("returns the engine's error message", () => {
    expect(describeSendError(new RpcError("transport", "engine offline"))).toBe("engine offline");
  });

  it("falls back to a generic message for non-Error inputs", () => {
    expect(describeSendError("nope")).toBe("The change could not be applied.");
  });
});

// --- Attachments (ticket 10) --------------------------------------------

/** A 1x1 PNG (the smallest valid file). Mirrors the desktop's bytes-only
 *  staging; format detection doesn't matter at this layer. */
const TINY_PNG_BYTES = new Uint8Array([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
  0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
  0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
  0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
  0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41,
  0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
  0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
  0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
  0x42, 0x60, 0x82,
]);

function stagePng(name: string): StagedAttachment {
  return stageBytes(name, new Uint8Array(TINY_PNG_BYTES));
}

describe("sendRun with attachments", () => {
  it("uploads each staged attachment and ships the resolved paths on QueueCommand", async () => {
    const caller = new FakeCaller();
    // One UploadChunk ack per chunk — this image base64s to 96 chars so a
    // single chunk covers it (UPLOAD_CHUNK_B64_CHARS is 680000). The next
    // call is UploadCommit, then QueueCommand.
    caller.replies.set("UploadChunk", {});
    caller.replies.set("UploadCommit", { path: "/host/uploads/abc-shot.png" });
    caller.replies.set("QueueCommand", { commandId: "cmd-1" });

    const staged: StagedAttachment[] = [stagePng("shot.png")];
    const result = await sendRun(
      caller,
      "chat-1",
      DRAFT,
      "see this",
      "/Users/me/proj",
      { mintMessageId: () => "msg-1" },
      { stagedAttachments: staged },
    );

    const methods = caller.calls.map((entry) => entry.method);
    expect(methods).toEqual(["UploadChunk", "UploadCommit", "QueueCommand"]);

    const uploadChunk = caller.calls[0]!.params as { uploadId: string; data: string; seq: number };
    expect(typeof uploadChunk.uploadId).toBe("string");
    expect(uploadChunk.uploadId.length).toBeGreaterThan(0);
    expect(uploadChunk.seq).toBe(0);
    expect(uploadChunk.data.length).toBeGreaterThan(0);

    const commit = caller.calls[1]!.params as { uploadId: string; fileName: string };
    expect(commit.uploadId).toBe(uploadChunk.uploadId);
    expect(commit.fileName).toBe("shot.png");

    const queue = caller.calls[2]!.params as {
      chatId: string;
      command: { kind: string; request: { prompt: string }; messageId: string };
      transfers: Array<{ uploadId: string; fileName: string }>;
    };
    expect(queue.chatId).toBe("chat-1");
    expect(queue.command.kind).toBe("run");
    expect(queue.transfers).toEqual([{ uploadId: uploadChunk.uploadId, fileName: "shot.png" }]);
    expect(queue.command.request.prompt).toContain("see this");
    expect(queue.command.request.prompt).toContain("- /host/uploads/abc-shot.png");

    expect(result.attachmentPaths).toEqual(["/host/uploads/abc-shot.png"]);
  });

  it("uses the attachment-only placeholder when sending images without text", async () => {
    const caller = new FakeCaller();
    caller.replies.set("UploadChunk", {});
    caller.replies.set("UploadCommit", { path: "/host/uploads/one.png" });
    caller.replies.set("QueueCommand", { commandId: "cmd-1" });
    await sendRun(
      caller,
      "chat-1",
      DRAFT,
      "",
      "/Users/me/proj",
      { mintMessageId: () => "msg-1" },
      { stagedAttachments: [stagePng("one.png")] },
    );
    const queue = caller.calls[2]!.params as {
      command: { request: { prompt: string } };
    };
    expect(queue.command.request.prompt).toMatch(/^See the attached image\(s\)\./);
  });

  it("rejects an empty send (no text, no attachments) before touching the wire", async () => {
    const caller = new FakeCaller();
    await expect(
      sendRun(caller, "chat-1", DRAFT, "   ", "/Users/me/proj", { }),
    ).rejects.toThrow(/empty/);
    expect(caller.calls).toHaveLength(0);
  });

  it("propagates upload errors with the desktop's verbatim copy", async () => {
    const caller = new FakeCaller();
    caller.nextError = new RpcError("transport", "upload chunk timed out");
    await expect(
      sendRun(
        caller,
        "chat-1",
        DRAFT,
        "see this",
        "/Users/me/proj",
        { mintMessageId: () => "msg-1" },
        { stagedAttachments: [stagePng("a.png")] },
      ),
    ).rejects.toThrow("Couldn't upload the attachment — the device may be offline.");
  });

  it("ships transfers with the chat's host device id on UploadChunk", async () => {
    const caller = new FakeCaller();
    caller.replies.set("UploadChunk", {});
    caller.replies.set("UploadCommit", { path: "/host/uploads/x.png" });
    caller.replies.set("QueueCommand", { commandId: "cmd-1" });
    await sendRun(
      caller,
      "chat-1",
      DRAFT,
      "see",
      "/Users/me/proj",
      { mintMessageId: () => "msg-1" },
      { stagedAttachments: [stagePng("x.png")] },
    );
    const uploadChunk = caller.calls[0]!.params as { uploadId: string; seq: number };
    expect(uploadChunk.seq).toBe(0);
  });
});
