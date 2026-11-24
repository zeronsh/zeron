// The identity lane's third probe of the mark rule, at 5af502d21, kept as written.
import { describe, expect, it } from "vitest";
import { markClaim } from "./listing";
const CLAIMS: [string, string][] = [
  ["variation selector between letters", "Veri️fied Focus"],
  ["a variation selector 17+ between letters", "Veri\u{e0100}fied Focus"],
  ["dot and space", "V. E. R. I. F. I. E. D. Focus"],
  ["middle dots", "V·E·R·I·F·I·E·D"],
  ["en dashes", "V–E–R–I–F–I–E–D"],
  ["two spaces", "V  E  R  I  F  I  E  D"],
  ["no-break spaces", "V E R I F I E D"],
  ["ideographic-width spaces", "V　E　R　I　F　I　E　D"],
  ["spaced lookalikes", "V E R І F І E D"],
];
describe("more spellings (5af502d21)", () => {
  for (const [what, name] of CLAIMS) it(`refuses ${what}: ${JSON.stringify(name)}`, () => expect(markClaim(name)).toBeDefined());
});
