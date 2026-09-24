import { afterEach, describe, expect, it, vi } from "vitest";
import { mintId } from "../src/lib/id";

/**
 * `mintId`'s three arms (§2.5): `crypto.randomUUID` in secure contexts;
 * a standards-shaped v4 from `getRandomValues` on plain-HTTP LAN origins
 * (where randomUUID is undefined); a collision-unlikely fallback when
 * neither exists.
 */

const UUID_SHAPE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("mintId", () => {
  it("delegates to crypto.randomUUID in a secure context", () => {
    vi.stubGlobal("crypto", { randomUUID: () => "11111111-2222-3333-4444-555555555555" });
    expect(mintId()).toBe("11111111-2222-3333-4444-555555555555");
  });

  it("mints a v4 from getRandomValues when randomUUID is undefined (plain-HTTP LAN)", () => {
    // Every byte 0xff: only the forced version/variant nibbles may differ.
    vi.stubGlobal("crypto", {
      getRandomValues: (bytes: Uint8Array) => {
        bytes.fill(0xff);
        return bytes;
      },
    });
    expect(mintId()).toBe("ffffffff-ffff-4fff-bfff-ffffffffffff");
  });

  it("shapes a random getRandomValues arm as a standards v4 (version 4, variant 10)", () => {
    vi.stubGlobal("crypto", {
      getRandomValues: (bytes: Uint8Array) => {
        for (let ix = 0; ix < bytes.length; ix += 1) {
          bytes[ix] = Math.floor(Math.random() * 256);
        }
        return bytes;
      },
    });
    const first = mintId();
    const second = mintId();
    expect(first).toMatch(UUID_SHAPE);
    expect(second).toMatch(UUID_SHAPE);
    // The version nibble is 4 and the variant bits are 10.
    expect(first[14]).toBe("4");
    expect(["8", "9", "a", "b"]).toContain(first[19]);
    expect(first).not.toBe(second);
  });

  it("falls back to a collision-unlikely local id when neither exists", () => {
    vi.stubGlobal("crypto", {});
    const first = mintId();
    const second = mintId();
    expect(first).toMatch(/^id-/);
    expect(second).toMatch(/^id-/);
    expect(first).not.toBe(second);
    expect(first.length).toBeGreaterThan("id-".length + 4);
  });
});
