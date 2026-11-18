import { describe, expect, it } from "vitest";
import { expectBodyHash, readBody } from "./body";

const post = (body: BodyInit, headers: Record<string, string> = {}) => new Request("https://edge.test/x", { method: "POST", body, headers });

/** A body that arrives in `chunks` pieces of `size` bytes, with no length declared. */
const streamed = (chunks: number, size: number) =>
  new Request("https://edge.test/x", {
    method: "POST",
    body: new ReadableStream<Uint8Array>({
      start(controller) {
        for (let i = 0; i < chunks; i++) controller.enqueue(new Uint8Array(size).fill(i));
        controller.close();
      }
    }),
    // @ts-expect-error Node's fetch needs this for a stream body.
    duplex: "half"
  });

describe("readBody", () => {
  it("reads once and answers the same bytes every time", async () => {
    const request = post("hello");
    const first = await readBody(request, 100);
    expect(new TextDecoder().decode(first as Uint8Array)).toBe("hello");
    expect(request.bodyUsed).toBe(true);
    expect(await readBody(request, 100)).toBe(first);
  });

  it("refuses a body past its cap, declared or streamed", async () => {
    expect(await readBody(post("x".repeat(11)), 10)).toBe("too_large");
    expect(await readBody(post("x", { "content-length": "999" }), 10)).toBe("too_large");
    expect(await readBody(streamed(3, 8), 20)).toBe("too_large");
    const fits = await readBody(streamed(3, 8), 24);
    expect((fits as Uint8Array).byteLength).toBe(24);
    expect([...(fits as Uint8Array).subarray(8, 10)]).toEqual([1, 1]);
  });

  it("answers too large to a later, smaller cap", async () => {
    const request = post("x".repeat(50));
    expect(((await readBody(request, 100)) as Uint8Array).byteLength).toBe(50);
    expect(await readBody(request, 10)).toBe("too_large");
  });

  it("answers a signed request's body only when it's the one signed", async () => {
    const hex = async (text: string) =>
      [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)))].map((b) => b.toString(16).padStart(2, "0")).join("");
    const good = post("pkg");
    expectBodyHash(good, await hex("pkg"));
    expect(new TextDecoder().decode((await readBody(good)) as Uint8Array)).toBe("pkg");
    const swapped = post("evil");
    expectBodyHash(swapped, await hex("pkg"));
    expect(await readBody(swapped)).toBe("body_mismatch");
    expect(await readBody(swapped)).toBe("body_mismatch");
    const emptied = new Request("https://edge.test/x", { method: "POST" });
    expectBodyHash(emptied, await hex("pkg"));
    expect(await readBody(emptied)).toBe("body_mismatch");
  });

  it("reads an empty body", async () => {
    expect((await readBody(new Request("https://edge.test/x", { method: "POST" }), 10)) as Uint8Array).toEqual(new Uint8Array(0));
  });
});
