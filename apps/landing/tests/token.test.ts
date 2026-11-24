// The fake edge makes and reads deploy tokens as the store does: held to the
// engine's vectors (docs/plugin-package-vectors.json `deploy_token` at
// plugin-store/engine 5866e77da, its tokens and reads, taken unedited).
import { describe, expect, test } from "vitest";
import { makeToken, readToken, tokenHash } from "../fake-edge/token.ts";
import vectors from "./vectors/deploy-token.json";

const bytes = (hex: string) => new Uint8Array(hex.match(/../g)!.map((h) => parseInt(h, 16)));

describe("a deploy token, as the engine's vectors have it", () => {
  for (const v of vectors.tokens)
    test(`${v.short}: made, named and kept as the store does`, async () => {
      const made = makeToken(bytes(v.random_hex));
      expect(made.token).toBe(v.token);
      expect(made.short).toBe(v.short);
      expect(await tokenHash(made.token)).toBe(v.sha256);
    });
  for (const r of vectors.reads)
    test(`read: ${JSON.stringify(r.token).slice(0, 30)}…`, () => {
      const read = readToken(r.token);
      expect(read.ok).toBe(r.ok);
      if (read.ok && r.ok) expect(read.short).toBe(r.short);
      if (!read.ok && !r.ok) expect(read.error).toBe(r.error);
    });
});
