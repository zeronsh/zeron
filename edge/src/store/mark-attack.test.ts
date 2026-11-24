// The identity lane's probes of the generated mark rule at c94c31eb2, kept as written: each failed then.
import { describe, expect, it } from "vitest";
import { checkListing, markClaim } from "./listing";
import { isText } from "./checks";
import { parseJsonText, type JsonObject } from "./json";
import { readManifest } from "./manifest";
const files = new Map(Object.entries({ "init.luau": "x", "README.md": "# Hi", "icon.svg": "<svg/>" }).map(([k, s]) => [k, new TextEncoder().encode(s)]));
const manifestNamed = (name: string) =>
  readManifest(parseJsonText(JSON.stringify({ id: "focus", name, version: "1.0.0", entry: "init.luau", description: "Focus.", icon: "icon.svg", categories: ["productivity"] })) as JsonObject);

// Each reads as a claim at a glance in ordinary sans-serif type; each should be refused.
const CLAIMS: [string, string][] = [
  ["lowercase l for I in capitals", "VERlFlED Focus"],
  ["lowercase l for I: OFFICIAL", "OFFlClAL Git"],
  ["capital I for l in lowercase", "The officiaI client"],
  ["Hangul choseong filler between letters (default-ignorable, Lo)", "Veriᅟfied Focus"],
  ["Hangul jungseong filler", "Veriᅠfied Focus"],
  ["Hangul filler", "Veriㅤfied Focus"],
  ["halfwidth Hangul filler", "Veriﾠfied Focus"],
  ["letter-spaced", "V E R I F I E D"],
  ["dotted capitals", "V.E.R.I.F.I.E.D. Focus"],
  ["cl for d", "verifiecl tools"],
];

describe("names that claim the mark (c94c31eb2)", () => {
  for (const [what, name] of CLAIMS) {
    it(`refuses ${what}: ${JSON.stringify(name)}`, () => {
      expect(markClaim(name)).toBeDefined();
    });
  }
});

describe("default-ignorable letters in a name at all", () => {
  for (const ch of ["ᅟ", "ᅠ", "ㅤ", "ﾠ"]) {
    it(`U+${ch.codePointAt(0)!.toString(16).toUpperCase()} is refused as unseen, as Cf is`, () => {
      const { problems } = checkListing(manifestNamed(`Focus${ch}Board`), files, isText);
      expect(problems.some((p) => p.field === "name")).toBe(true);
    });
  }
});
