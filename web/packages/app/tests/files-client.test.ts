import { describe, expect, it } from "vitest";
import {
  MAX_WORKSPACE_IMAGE_BYTES,
  WORKSPACE_IMAGE_CHUNK_BYTES,
  type WorkspaceImageChunk,
} from "@zeron/proto";
import { methods } from "@zeron/engine-client";
import { WorkspaceFilesClient, type FilesCaller } from "../src/lib/files-client";

function encode(bytes: number[]): string {
  return btoa(String.fromCharCode(...bytes));
}

function chunk(fields: Partial<WorkspaceImageChunk>): WorkspaceImageChunk {
  return {
    checkoutId: "checkout-1",
    contentHash: "hash-1",
    mimeType: "image/png",
    data: encode([1, 2, 3]),
    nextOffset: 3,
    size: 3,
    done: true,
    ...fields,
  };
}

function caller(respond: (method: string, params: Record<string, unknown>) => unknown): FilesCaller & { calls: { method: string; params: Record<string, unknown> }[] } {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  return {
    calls,
    call<T>(method: string, params?: unknown): Promise<T> {
      const request = (params ?? {}) as Record<string, unknown>;
      calls.push({ method, params: request });
      return Promise.resolve(respond(method, request) as T);
    },
  };
}

describe("WorkspaceFilesClient", () => {
  it("the Git status watch targets the chat and carries the wire frame shape", () => {
    const transport = caller(() => ({ status: null }));
    const client = new WorkspaceFilesClient(transport, { chatId: "chat-1" });
    const seen: { onItem: (frame: unknown) => void }[] = [];
    const engine = {
      watch(
        method: string,
        params: unknown,
        handlers: { onItem: (frame: unknown) => void },
      ): { cancel(): void } {
        expect(method).toBe(methods.WATCH_WORKSPACE_GIT_STATUS);
        expect(params).toEqual({ chatId: "chat-1" });
        seen.push(handlers);
        return { cancel: () => undefined };
      },
    };
    const handle = client.watchGitStatus(
      engine as unknown as Parameters<typeof client.watchGitStatus>[0],
      { onItem: () => undefined },
    );
    expect(seen).toHaveLength(1);
    handle.cancel();
  });

  it("flattens the space target into every request (serde flatten parity)", async () => {
    const transport = caller(() => ({ directory: "", entries: [], truncated: false }));
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    await client.listDirectory("src", true);
    expect(transport.calls[0]).toEqual({
      method: methods.LIST_WORKSPACE_DIRECTORY,
      params: { spaceId: "space-1", directory: "src", includeIgnored: true },
    });
    await client.search("main", false);
    expect(transport.calls[1]).toEqual({
      method: methods.SEARCH_WORKSPACE_FILES,
      params: { spaceId: "space-1", query: "main", includeIgnored: false, limit: 200 },
    });
    await client.readFile("src/lib.rs");
    expect(transport.calls[2]).toEqual({
      method: methods.READ_WORKSPACE_FILE,
      params: { spaceId: "space-1", path: "src/lib.rs" },
    });
  });

  it("passes write requests through with the target", async () => {
    const transport = caller(() => ({ status: "written", file: { path: "a.ts", contentHash: "h2", size: 3 } }));
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    const outcome = await client.writeFile({
      expectedCheckoutId: "checkout-1",
      path: "a.ts",
      text: "one",
      expectedContentHash: "h1",
      encoding: "utf8",
      lineEnding: "lf",
    });
    expect(transport.calls[0]?.method).toBe(methods.WRITE_WORKSPACE_FILE);
    expect(transport.calls[0]?.params).toMatchObject({ spaceId: "space-1", expectedCheckoutId: "checkout-1" });
    expect(outcome.status).toBe("written");
  });

  it("refuses an image read without a checkout identity", async () => {
    const client = new WorkspaceFilesClient(caller(() => chunk({})), { spaceId: "space-1" });
    await expect(client.readImage("logo.png", "")).rejects.toThrow("checkout identity unavailable");
  });

  it("assembles a single-chunk image", async () => {
    const client = new WorkspaceFilesClient(caller(() => chunk({})), { spaceId: "space-1" });
    const image = await client.readImage("logo.png", "checkout-1");
    expect(image.mimeType).toBe("image/png");
    expect([...image.bytes]).toEqual([1, 2, 3]);
  });

  it("assembles a multi-chunk image, pinning hash and mime after the first", async () => {
    const requests: Record<string, unknown>[] = [];
    const transport = caller((_method, params) => {
      requests.push(params);
      if (params.offset === 0) {
        return chunk({ data: encode([1, 2]), nextOffset: 2, size: 5, done: false });
      }
      return chunk({ data: encode([3, 4, 5]), nextOffset: 5, size: 5, done: true });
    });
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    const image = await client.readImage("logo.png", "checkout-1");
    expect([...image.bytes]).toEqual([1, 2, 3, 4, 5]);
    expect(requests[1]).toMatchObject({ offset: 2, expectedContentHash: "hash-1" });
  });

  it("rejects a chunk whose identity changed mid-read", async () => {
    const transport = caller((_method, params) =>
      params.offset === 0
        ? chunk({ data: encode([1, 2]), nextOffset: 2, size: 5, done: false })
        : chunk({ contentHash: "other", data: encode([3, 4, 5]), nextOffset: 5, size: 5, done: true }),
    );
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    await expect(client.readImage("logo.png", "checkout-1")).rejects.toThrow("identity or size changed");
  });

  it("rejects a chunk with a bad offset", async () => {
    const transport = caller(() => chunk({ nextOffset: 99, done: false }));
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    await expect(client.readImage("logo.png", "checkout-1")).rejects.toThrow("chunk offset");
  });

  it("rejects an image beyond the size cap", async () => {
    const transport = caller(() => chunk({ size: MAX_WORKSPACE_IMAGE_BYTES + 1 }));
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    await expect(client.readImage("logo.png", "checkout-1")).rejects.toThrow("identity or size changed");
  });

  it("stops after the chunk limit", async () => {
    const maxChunks = Math.floor(MAX_WORKSPACE_IMAGE_BYTES / WORKSPACE_IMAGE_CHUNK_BYTES);
    let calls = 0;
    const transport = caller(() => {
      calls += 1;
      // Never done; size large enough that nextOffset never reaches it, but
      // under the cap so identity validation passes.
      return chunk({ data: encode([1]), nextOffset: calls, size: MAX_WORKSPACE_IMAGE_BYTES, done: false });
    });
    const client = new WorkspaceFilesClient(transport, { spaceId: "space-1" });
    await expect(client.readImage("logo.png", "checkout-1")).rejects.toThrow("chunk limit exceeded");
    expect(calls).toBe(maxChunks + 1);
    expect(WORKSPACE_IMAGE_CHUNK_BYTES).toBeGreaterThan(0);
  });
});
