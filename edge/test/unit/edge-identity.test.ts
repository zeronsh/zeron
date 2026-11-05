import { describe, expect, it } from "vitest";
import { cleanJpeg, cleanPng, crc32 } from "../../src/image-check";
import { nameProblem, orgNameProblem } from "../../src/names";
import { isReservedUsername } from "../../src/username";

describe("reserved usernames (R17)", () => {
  it("reserves brands, staff and lookalikes", () => {
    for (const name of [
      "github", "anthropic", "openai", "google", "apple", "microsoft", "wing", "winglee", "wingleeio", "admin", "support",
      "security", "zeron", "zer0n", "zeronhq", "the-zeron", "zer_on", "g1thub", "githubbot", "openai-dev", "anthropic-ai",
      "google-dev", "apple1", "adm1n", "admin-bob", "bob-admin", "support-2", "acme-support", "official", "acme-official",
      "claude", "claude-bot", "cloudflare", "rnicrosoft", "staff", "verified", "username", "user-name", "noreply"
    ]) {
      expect(isReservedUsername(name), name).toBe(true);
    }
  });

  it("leaves ordinary names alone", () => {
    for (const name of ["ada", "pineapple", "bigapple", "steam", "wingspan-fan", "keyowner", "goneowner", "runnerowner", "grace_h", "lovelace", "supporter", "staffing", "helper-bot"]) {
      expect(isReservedUsername(name), name).toBe(false);
    }
  });
});

describe("names in emails (R24)", () => {
  it("refuses links, addresses and invisible characters in any name", () => {
    for (const name of ["see evil.com", "https://x", "www.x", "me@x", "a/b", "<b>", "x​y", "a‮b", "a b", "evil．com"]) {
      expect(nameProblem(name), name).toBeDefined();
    }
    for (const name of ["Ada Lovelace", "J. R. Tolkien", "Ada \t Lovelace", "Søren Kierkegaard", "李小龙", "O'Brien-Smith"]) {
      expect(nameProblem(name), name).toBeUndefined();
    }
  });

  it("refuses mixed alphabets and our or vendors' names in org names", () => {
    expect(orgNameProblem("Pаypal")).toBe("mixed_scripts");
    expect(orgNameProblem("Zeron")).toBe("impersonation");
    expect(orgNameProblem("ZERON billing")).toBe("impersonation");
    expect(orgNameProblem("Zer0n")).toBe("impersonation");
    expect(orgNameProblem("Ζеrοn")).toBeDefined();
    expect(orgNameProblem("Git Hub Inc")).toBe("impersonation");
    expect(orgNameProblem("Apple")).toBe("impersonation");
    expect(orgNameProblem("Acme\tCorp")).toBe("characters");
    for (const name of ["Acme", "Pineapple Studio", "株式会社 LINE", "Société Générale", "Анара"]) {
      expect(orgNameProblem(name), name).toBeUndefined();
    }
  });
});

const PNG = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="),
  (c) => c.charCodeAt(0)
);
const join = (...parts: ArrayLike<number>[]) => Uint8Array.from(parts.flatMap((p) => Array.from(p)));
const chunk = (type: string, data: number[]) => {
  const body = join(new TextEncoder().encode(type), data);
  const crc = crc32(body);
  return join([0, 0, data.length >> 8, data.length & 0xff], body, [crc >>> 24, (crc >>> 16) & 0xff, (crc >>> 8) & 0xff, crc & 0xff]);
};

describe("image checks (R24)", () => {
  it("keeps a plain PNG as it is, and drops metadata and trailers", () => {
    expect(cleanPng(PNG)).toEqual(PNG);
    const tagged = join(PNG.subarray(0, 33), chunk("tEXt", [65, 0, 66]), chunk("eXIf", [1, 2]), PNG.subarray(33), [1, 2, 3]);
    expect(cleanPng(tagged)).toEqual(PNG);
  });

  it("refuses broken or oversized PNGs", () => {
    const huge = PNG.slice();
    huge.set([0, 0, 0x40, 0x01], 16); // width 16385
    expect(cleanPng(huge)).toBeUndefined();
    expect(cleanPng(PNG.subarray(0, PNG.length - 12))).toBeUndefined(); // no IEND
    expect(cleanPng(join(PNG.subarray(0, 33), chunk("ABCD", [1]), PNG.subarray(33)))).toBeUndefined();
    expect(cleanPng(new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBeUndefined();
  });

  /** A structurally sound JPEG: SOI, APP0 JFIF, `extra` segments, DQT, SOF0
   * 1×1, DHT, SOS, data, EOI, then `trailer`. */
  const jpeg = (extra: number[] = [], trailer: number[] = [], width = 1) =>
    join(
      [0xff, 0xd8],
      [0xff, 0xe0, 0, 16, 0x4a, 0x46, 0x49, 0x46, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
      extra,
      [0xff, 0xdb, 0, 67, 0, ...new Array(64).fill(1)],
      [0xff, 0xc0, 0, 11, 8, 0, 1, width >> 8, width & 0xff, 1, 1, 0x11, 0],
      [0xff, 0xc4, 0, 20, 0, 1, ...new Array(15).fill(0), 0],
      [0xff, 0xda, 0, 8, 1, 1, 0, 0, 0x3f, 0],
      [0x12, 0xff, 0x00, 0x34],
      [0xff, 0xd9],
      trailer
    );

  it("drops EXIF, comments and trailers from a JPEG, keeping its orientation", () => {
    expect(cleanJpeg(jpeg())).toEqual(jpeg());
    const gps = new TextEncoder().encode("GPS 51.5N 0.12W");
    // APP1 EXIF (big-endian) with orientation 6 and some other tag, plus a comment.
    const tiff = [0x4d, 0x4d, 0, 0x2a, 0, 0, 0, 8, 0, 2, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
    const exifBody = [...new TextEncoder().encode("Exif"), 0, 0, ...tiff, ...gps];
    const app1 = [0xff, 0xe1, (exifBody.length + 2) >> 8, (exifBody.length + 2) & 0xff, ...exifBody];
    const com = [0xff, 0xfe, 0, 2 + gps.length, ...gps];
    const out = cleanJpeg(jpeg([...app1, ...com], [0x3c, 0x68]))!;
    expect(out).toBeDefined();
    const text = new TextDecoder("latin1").decode(out);
    expect(text).not.toContain("GPS");
    expect(text).toContain("Exif");
    expect(out.subarray(-2)).toEqual(new Uint8Array([0xff, 0xd9]));
    // Orientation 6 survives in the minimal EXIF.
    const at = text.indexOf("Exif");
    expect(out[at + 6 + 18]).toBe(6);
  });

  it("refuses JPEGs without a frame, too wide, or cut short", () => {
    expect(cleanJpeg(jpeg([], [], 5000))).toBeUndefined();
    const full = jpeg();
    expect(cleanJpeg(full.subarray(0, full.length - 2))).toBeUndefined();
    expect(cleanJpeg(new Uint8Array([0xff, 0xd8, 0xff, 0xd9]))).toBeUndefined();
  });
});
