import { describe, expect, it } from "vitest";
import { readCapped } from "./read-capped";

/** A body `size` bytes long, a chunk at a time, saying how much was pulled. */
const counted = (size: number, chunk = 64 * 1024) => {
  const seen = { pulled: 0 };
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (seen.pulled >= size) return controller.close();
      const n = Math.min(chunk, size - seen.pulled);
      seen.pulled += n;
      controller.enqueue(new Uint8Array(n).fill(7));
    }
  });
  return { body, seen };
};

const post = (body: ReadableStream<Uint8Array> | Uint8Array, length?: number): Request =>
  new Request("https://chat/checkpoint", {
    method: "POST",
    body,
    headers: length === undefined ? {} : { "content-length": String(length) },
    // (A stream body: Node's fetch asks for this.)
    duplex: "half"
  } as RequestInit);

describe("readCapped", () => {
  it("refuses a declared length past the cap before reading a byte", async () => {
    const { body, seen } = counted(40 * 1024 * 1024);
    expect(await readCapped(post(body, 40 * 1024 * 1024), 32 * 1024 * 1024)).toBeUndefined();
    expect(seen.pulled).toBeLessThanOrEqual(64 * 1024 * 2);
  });

  it("stops reading a body of no declared length once it passes the cap", async () => {
    const cap = 1024 * 1024;
    const { body, seen } = counted(40 * 1024 * 1024);
    expect(await readCapped(post(body), cap)).toBeUndefined();
    // (The chunk that passed it, and at most what the stream had ready.)
    expect(seen.pulled).toBeLessThan(cap + 4 * 64 * 1024);
  });

  it("reads a body within the cap whole, declared or not", async () => {
    const bytes = new Uint8Array(300_000).map((_, i) => i & 0xff);
    expect(await readCapped(post(bytes, bytes.length), bytes.length)).toEqual(bytes);
    const { body } = counted(200_000);
    const got = await readCapped(post(body), 200_000);
    expect(got?.byteLength).toBe(200_000);
    expect(await readCapped(post(new Uint8Array(0), 0), 10)).toEqual(new Uint8Array(0));
  });
});
