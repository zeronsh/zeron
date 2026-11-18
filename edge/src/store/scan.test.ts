/**
 * What a hostile body costs the scanner: each shape at the store's 20 MiB
 * cap, read as `verifyPackage` reads one, inside a CPU budget (Node; a
 * Worker's isolate runs the same V8). The numbers behind the budgets are in
 * docs/plugin-store-limits.md.
 */
import { describe, expect, it } from "vitest";
import { verifyPackage } from "./package";

/** The store's body cap (principal.ts MAX_PUBLISH_BODY_BYTES, amendment 34). */
const SIZE = 20 * 1024 * 1024;
const fill = (head: string, unit: string, tail: string): Uint8Array => {
  const n = Math.floor((SIZE - head.length - tail.length) / unit.length);
  return new TextEncoder().encode(head + unit.repeat(n) + tail);
};

/** Each shape, built to just under the cap. */
export const HOSTILE: Record<string, () => Uint8Array> = {
  "duplicate top-level keys": () => fill('{"a":0', ',"a":0', "}"),
  "top-level members of empty arrays": () => fill('{"a":[]', ',"a":[]', "}"),
  "files full of one key": () => fill('{"format":"zeron-plugin:v1","manifest":{},"files":{"a":""', ',"a":""', "}}"),
  "files full of distinct keys": () => {
    const head = '{"format":"zeron-plugin:v1","manifest":{},"files":{';
    const parts: string[] = [];
    let size = head.length + 2;
    for (let i = 0; size < SIZE - 64; i++) {
      const p = `"k${i}":"",`;
      parts.push(p);
      size += p.length;
    }
    return new TextEncoder().encode(head + parts.join("") + '"z":""}}');
  },
  "one 20 MB string of A": () => fill('{"x":"', "A", '"}'),
  "1e308 repeated": () => fill('{"x":[1e308', ",1e308", "]}"),
  "escaped file data": () => fill('{"format":"zeron-plugin:v1","manifest":{},"files":{"a":"', "\\/", '"}}'),
  "unicode escapes in file data": () => fill('{"format":"zeron-plugin:v1","manifest":{},"files":{"a":"', "\\u0041", '"}}'),
  "zeros": () => fill('{"x":[0', ",0", "]}"),
  "empty objects": () => fill('{"x":[{}', ",{}", "]}")
};

/** Milliseconds `verifyPackage` takes on `raw`, refusal or not. */
const timed = async (raw: Uint8Array): Promise<number> => {
  const started = performance.now();
  await verifyPackage(raw, SIZE + 1).catch(() => undefined);
  return performance.now() - started;
};

describe("hostile bodies at the 20 MiB cap stay within a CPU budget", () => {
  for (const [name, make] of Object.entries(HOSTILE)) {
    it(name, async () => {
      const raw = make();
      expect(raw.length).toBeLessThanOrEqual(SIZE);
      await timed(raw.subarray(0, 4096)); // warm
      const ms = await timed(raw);
      console.log(`scan budget: ${name}: ${ms.toFixed(0)} ms`);
      expect(ms).toBeLessThan(1500);
    }, 30_000);
  }
});
