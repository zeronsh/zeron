// The identity lane's probe of the variation-selector ruling and its neighbours, at 5af502d21, kept as written.
// (a) the word test runs with every default-ignorable removed, selectors included;
// (b) a name, summary or keyword holds U+FE0F only right after an Emoji character, and no other selector at all.
import { describe, expect, it } from "vitest";
import { checkListing, markClaim } from "./listing";
import { isText } from "./checks";
import { parseJsonText, type JsonObject } from "./json";
import { readManifest } from "./manifest";

const files = new Map(Object.entries({ "init.luau": "x", "README.md": "# Hi", "icon.svg": "<svg/>" }).map(([k, s]) => [k, new TextEncoder().encode(s)]));
const listing = (fields: Record<string, unknown>) =>
  checkListing(
    readManifest(parseJsonText(JSON.stringify({ id: "focus", name: "Focus", version: "1.0.0", entry: "init.luau", description: "Focus.", icon: "icon.svg", categories: ["productivity"], ...fields })) as JsonObject),
    files,
    isText
  ).problems.map((p) => p.field);

describe("(a) no selector splits a mark word", () => {
  for (const [what, text] of [
    ["VS16 inside", "Veri️fied"],
    ["VS15 inside", "Veri︎fied"],
    ["VS1 inside", "Veri︀fied"],
    ["a supplement selector inside", "Veri\u{e0100}fied"],
  ] as const) it(`claims: ${what}`, () => expect(markClaim(text)).toBeDefined());
});

describe("(b) where a selector may stand", () => {
  for (const field of ["name", "description"] as const) {
    const bad: [string, string][] = [
      ["VS16 after a plain letter", "Focus️ board"],
      ["VS15 after an emoji", "Focus ⭐︎"],
      ["VS1", "Focus︀"],
      ["a supplement selector", "Focus\u{e0101}"],
      ["VS16 twice", "Focus ⭐️️"],
      ["VS16 at the start", "️Focus"],
    ];
    for (const [what, text] of bad) it(`${field} refuses ${what}`, () => expect(listing({ [field]: text })).toContain(field));
    it(`${field} keeps "⭐️" (an emoji with VS16)`, () => expect(listing({ [field]: "Focus ⭐️" })).not.toContain(field));
  }
  it("keywords refuse VS16 after a plain letter", () => expect(listing({ keywords: ["timer️"] })).toContain("keywords"));
});

describe("what else the exception and its neighbours let in", () => {
  // Enclosing marks (Me) survive the fold, which drops only Mn: they split a word.
  it("claims: an enclosing keycap mark inside", () => expect(markClaim("Veri⃣fied")).toBeDefined());
  it("claims: an enclosing circle inside", () => expect(markClaim("Veri⃝fied")).toBeDefined());
  // Emoji letters NFKC leaves alone, each reading as its letter in a box.
  it("claims: negative circled capitals", () => expect(markClaim("\u{1f165}\u{1f154}\u{1f161}\u{1f158}\u{1f155}\u{1f158}\u{1f154}\u{1f153} Focus")).toBeDefined());
  it("claims: negative squared capitals", () => expect(markClaim("\u{1f185}\u{1f174}\u{1f181}\u{1f178}\u{1f175}\u{1f178}\u{1f174}\u{1f173} Focus")).toBeDefined());
  it("claims: spaced regional indicators", () => expect(markClaim("\u{1f1fb} \u{1f1ea} \u{1f1f7} \u{1f1ee} \u{1f1eb} \u{1f1ee} \u{1f1ea} \u{1f1e9}")).toBeDefined());
  // A ZWJ sequence and a tag sequence: refused as Cf today (the ruling allows only FE0F).
  it("ZWJ in a name is refused (👩‍💻)", () => expect(listing({ name: "\u{1f469}‍\u{1f4bb} Dev" })).toContain("name"));
  it("tag characters in a name are refused (a subdivision flag)", () => expect(listing({ name: "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f} Focus" })).toContain("name"));
  it("a keycap (1 FE0F 20E3) is kept", () => expect(listing({ name: "Top 1️⃣ timer" })).not.toContain("name"));
});
