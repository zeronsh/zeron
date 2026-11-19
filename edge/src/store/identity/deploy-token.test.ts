import { describe, expect, it } from "vitest";
import {
  DEPLOY_TOKEN_ID_RE,
  crc32,
  deployTokenHash,
  looksLikeDeployToken,
  newDeployToken,
  newDeployTokenId,
  parseDeployToken,
  tokenChecksum,
  tokenFromBody
} from "./deploy-token";

describe("deploy tokens' format", () => {
  it("computes CRC-32 as zlib does", () => {
    const ascii = (s: string) => new TextEncoder().encode(s);
    expect(crc32(ascii(""))).toBe(0);
    expect(crc32(ascii("123456789"))).toBe(0xcbf43926);
    expect(crc32(ascii("The quick brown fox jumps over the lazy dog"))).toBe(0x414fa339);
  });

  it("makes zdt_ + 43 base62 characters of 32 random bytes + a 6-character checksum", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 200; i++) {
      const token = newDeployToken();
      expect(token).toMatch(/^zdt_[0-9A-Za-z]{49}$/);
      const body = token.slice(4, 47);
      expect(token.slice(47)).toBe(tokenChecksum(body));
      expect(parseDeployToken(token)).toEqual({ short: body.slice(0, 8) });
      seen.add(token);
    }
    expect(seen.size).toBe(200);
  });

  it("writes the body and checksum left-padded with 0, the checksum the CRC-32 of the ASCII zdt_ + body", () => {
    const body = "0".repeat(43);
    const crc = crc32(new TextEncoder().encode(`zdt_${body}`));
    let n = crc;
    let digits = "";
    const alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    while (n > 0) {
      digits = alphabet[n % 62]! + digits;
      n = Math.floor(n / 62);
    }
    expect(tokenFromBody(body)).toBe(`zdt_${body}${digits.padStart(6, "0")}`);
    // 62^6 > 2^32: every checksum fits.
    expect(62 ** 6).toBeGreaterThan(2 ** 32);
    // 2^256 - 1 is 43 characters in base62.
    expect(62 ** 43).toBeGreaterThan(2 ** 256);
    expect(62 ** 42).toBeLessThan(2 ** 256);
  });

  it("reads only a whole, well-formed token whose checksum is right", () => {
    const token = newDeployToken();
    const flip = (s: string, i: number) => `${s.slice(0, i)}${s[i] === "A" ? "B" : "A"}${s.slice(i + 1)}`;
    for (const bad of [
      "",
      "zdt_",
      token.slice(0, -1),
      `${token}0`,
      `ZDT_${token.slice(4)}`,
      `zdk_${token.slice(4)}`,
      ` ${token}`,
      `${token}\n`,
      `${token.slice(0, 20)}-${token.slice(21)}`,
      flip(token, 10),
      flip(token, 50)
    ]) {
      expect(parseDeployToken(bad), JSON.stringify(bad)).toBeUndefined();
    }
  });

  it("knows a token by its prefix alone, so no route reads one as a session", () => {
    expect(looksLikeDeployToken("zdt_")).toBe(true);
    expect(looksLikeDeployToken("zdt_anything")).toBe(true);
    expect(looksLikeDeployToken("Zdt_x")).toBe(false);
    expect(looksLikeDeployToken("abc")).toBe(false);
  });

  it("stores the SHA-256 in hex, and ids its own way", async () => {
    expect(await deployTokenHash("abc")).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    const ids = new Set(Array.from({ length: 100 }, newDeployTokenId));
    expect(ids.size).toBe(100);
    for (const id of ids) expect(id).toMatch(DEPLOY_TOKEN_ID_RE);
  });
});
