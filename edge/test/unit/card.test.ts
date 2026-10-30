import { describe, expect, it } from "vitest";
import vectors from "../../../docs/card-vectors.json";
import { CARD_SCENES, HOLOS, cardFirstName, cardHash, deriveCard } from "../../src/card";
import { isPublicCardPath, publicCardGate } from "../../src/card-gate";
import { foldUsername, isReservedUsername } from "../../src/username";

describe("card function", () => {
  it("names the holos in the published order", () => {
    expect(HOLOS).toEqual(vectors.holos);
  });

  it.each(vectors.hashes)("hashes $input", ({ input, hash }) => {
    expect(cardHash(input)).toBe(hash);
  });

  it.each(vectors.cards)("derives the card of $id from $scenes scenes", ({ id, scenes, scene, holo, holoName, sky }) => {
    expect(vectors.holos[holo]).toBe(holoName);
    expect(deriveCard(id, scenes)).toEqual({ scene, holo: holoName, sky });
  });

  it("covers today's scene count", () => {
    expect(vectors.cards.some((c) => c.scenes === CARD_SCENES)).toBe(true);
  });
});

describe("username rules", () => {
  it("folds case and keeps the allowed characters", () => {
    expect(foldUsername("Wing")).toBe("wing");
    expect(foldUsername("a_b-c")).toBe("a_b-c");
    expect(foldUsername("0x7")).toBe("0x7");
    expect(foldUsername("a".repeat(20))).toBe("a".repeat(20));
  });

  it("refuses anything else", () => {
    for (const bad of ["", "ab", "a".repeat(21), "_ab", "-ab", "a b", "a.b", "a/b", "a@b", "wing\n", "wïng", "ａｂｃ", undefined, null, 123, ["wing"]]) {
      expect(foldUsername(bad)).toBeUndefined();
    }
  });

  it("doesn't let a non-ASCII letter fold into a name", () => {
    // U+212A KELVIN SIGN lowercases to an ASCII k.
    expect("Kelly".toLowerCase()).toBe("kelly");
    expect(foldUsername("Kelly")).toBeUndefined();
  });

  it("reserves routes and our own name, however they're spelled", () => {
    for (const name of ["admin", "api", "support", "zeron", "ad-min", "sup_port", "zeron-team", "zeronofficial", "z_eron"]) {
      expect(isReservedUsername(name)).toBe(true);
    }
    for (const name of ["wing", "ada", "supporter"]) expect(isReservedUsername(name)).toBe(false);
  });
});

describe("a card's first name", () => {
  it("is the first word of the first name, and nothing of the surname", () => {
    expect(cardFirstName("Grace", "grace")).toBe("Grace");
    expect(cardFirstName("  Ada  ", "ada")).toBe("Ada");
    expect(cardFirstName("Ada Lovelace", "ada")).toBe("Ada");
    expect(cardFirstName("Ada L.", "ada")).toBe("Ada");
    expect(cardFirstName("Jean-Luc", "jl")).toBe("Jean-Luc");
    expect(cardFirstName("Mary\tAnn", "mary")).toBe("Mary");
    expect(cardFirstName("\u200BZoë", "zoe")).toBe("Zoë");
  });

  it("falls back to the username", () => {
    for (const none of [null, "", "   ", "\u200B\u0000"]) expect(cardFirstName(none, "wing")).toBe("wing");
  });

  it("is capped", () => {
    expect([...cardFirstName("x".repeat(100), "u")]).toHaveLength(32);
    expect(cardFirstName("👩‍💻".repeat(40), "u").length).toBeLessThanOrEqual(64);
  });
});

describe("the public card gate", () => {
  const serve = async () => new Response("ok");
  const at = (path: string) => new URL(`https://edge.test${path}`);

  it("covers the page, anything under it, and the JSON", () => {
    for (const p of ["/@wing", "/%40wing", "/@wing/player", "/@wing/card.png", "/cards/wing", "/cards", "/CARDS/wing"]) {
      expect(isPublicCardPath(p)).toBe(true);
    }
    for (const p of ["/", "/cardsx", "/username", "/admin/usernames/set", "/wing@x"]) expect(isPublicCardPath(p)).toBe(false);
  });

  it("passes other routes by", async () => {
    expect(await publicCardGate(new Request("https://edge.test/username"), {}, at("/username"), serve)).toBeUndefined();
  });

  it("hides what went wrong behind it", async () => {
    const res = await publicCardGate(new Request("https://edge.test/@wing"), {}, at("/@wing"), async () => {
      throw new Error("SQLITE_ERROR: no such column user_id at user-directory.ts:480");
    });
    expect(res!.status).toBe(503);
    expect(await res!.text()).toBe('{"error":"unavailable"}');
  });

  it("limits by the client's IP", async () => {
    const keys: string[] = [];
    const CARD_READS = { limit: async ({ key }: { key: string }) => (keys.push(key), { success: key !== "198.51.100.1" }) };
    const req = (ip: string) => new Request("https://edge.test/cards/wing", { headers: { "cf-connecting-ip": ip } });
    expect((await publicCardGate(req("203.0.113.1"), { CARD_READS }, at("/cards/wing"), serve))!.status).toBe(200);
    expect((await publicCardGate(req("198.51.100.1"), { CARD_READS }, at("/cards/wing"), serve))!.status).toBe(429);
    expect(keys).toEqual(["203.0.113.1", "198.51.100.1"]);
  });
});
